//! Sign-in with Spotify's Authorization Code flow with PKCE (no client
//! secret). A short-lived listener on 127.0.0.1 catches the browser redirect.
//! Token refresh uses the saved refresh token. The plugin keeps the tokens in
//! its global settings.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Scopes needed by the actions (see `docs/PLUGIN_SPOTIFY.md`).
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-library-read user-library-modify playlist-read-private playlist-read-collaborative playlist-modify-private playlist-modify-public";
const REDIRECT_PORT: u16 = 57140;
const REDIRECT_URI: &str = "http://127.0.0.1:57140/callback";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const ME_URL: &str = "https://api.spotify.com/v1/me";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// A connection that sends nothing (browser preconnect) must not block the listener.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Refresh the access token when less than this many seconds are left.
const REFRESH_MARGIN: u64 = 60;
const SUCCESS_PAGE: &str = "<!doctype html><html lang=\"de\"><meta charset=\"utf-8\"><title>OpenDeckN3</title><body style=\"font-family:sans-serif;padding:2em\"><p>Anmeldung erfolgreich – du kannst dieses Fenster schließen.</p></body></html>";
const FAILURE_PAGE: &str = "<!doctype html><html lang=\"de\"><meta charset=\"utf-8\"><title>OpenDeckN3</title><body style=\"font-family:sans-serif;padding:2em\"><p>Anmeldung fehlgeschlagen – zurück zu OpenDeckN3 und erneut „Anmelden“ klicken.</p></body></html>";

#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

/// The saved sign-in is rejected; the user has to sign in again.
#[derive(Debug)]
pub struct ReLogin;

impl std::fmt::Display for ReLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bitte neu anmelden")
    }
}

impl std::error::Error for ReLogin {}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

pub fn needs_refresh(expires_at: u64, now: u64) -> bool {
    expires_at < now + REFRESH_MARGIN
}

/// Random URL-safe string from `bytes` random bytes (PKCE verifier, OAuth state).
fn random_token(bytes: usize) -> anyhow::Result<String> {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).map_err(|e| anyhow!("no randomness: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

/// S256 code challenge of a verifier (RFC 7636, section 4.2).
pub fn challenge_of(verifier: &str) -> String {
    use sha2::Digest;
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn authorize_url(client_id: &str, challenge: &str, state: &str) -> anyhow::Result<String> {
    let url = reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        [
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT_URI),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge),
            ("state", state),
            ("scope", SCOPES),
        ],
    )?;
    Ok(url.to_string())
}

fn reason(body: &Value) -> &str {
    body["error_description"]
        .as_str()
        .or(body["error"].as_str())
        .unwrap_or_default()
}

async fn body_json(res: reqwest::Response) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&res.bytes().await?).unwrap_or_default())
}

fn tokens_from(body: &Value, now: u64) -> Option<Tokens> {
    Some(Tokens {
        access_token: body["access_token"].as_str()?.to_owned(),
        refresh_token: body["refresh_token"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        expires_at: now + body["expires_in"].as_u64().unwrap_or(3600),
    })
}

/// Spotify sends a new refresh token only sometimes; the old one stays valid otherwise.
fn keep_refresh_token(mut tokens: Tokens, old: &str) -> Tokens {
    if tokens.refresh_token.is_empty() {
        tokens.refresh_token = old.to_owned();
    }
    tokens
}

/// Signs in through the browser. `show_url` receives the authorize URL once
/// the callback listener is ready. The listener is closed when this returns.
pub async fn sign_in(
    http: &reqwest::Client,
    client_id: &str,
    show_url: impl FnOnce(String),
) -> anyhow::Result<Tokens> {
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
        .await
        .with_context(|| format!("Port {REDIRECT_PORT} belegt – läuft schon eine Anmeldung?"))?;
    let verifier = random_token(48)?;
    let state = random_token(16)?;
    show_url(authorize_url(client_id, &challenge_of(&verifier), &state)?);
    let code = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_code(listener, &state))
        .await
        .context("Zeitlimit abgelaufen – erneut „Anmelden“ klicken")??;
    exchange(http, client_id, &code, &verifier).await
}

/// Accepts connections until the redirect to `/callback` arrives.
async fn wait_for_code(listener: TcpListener, state: &str) -> anyhow::Result<String> {
    loop {
        let (mut socket, _) = listener.accept().await.context("Callback-Port")?;
        let line = match tokio::time::timeout(REQUEST_TIMEOUT, request_line(&mut socket)).await {
            Ok(Ok(line)) => line,
            _ => String::new(),
        };
        match parse_request_line(&line) {
            Callback::Other => respond(&mut socket, "404 Not Found", "").await,
            Callback::Answer(answer) => {
                let result = check_answer(answer, state);
                let page = if result.is_ok() {
                    SUCCESS_PAGE
                } else {
                    FAILURE_PAGE
                };
                respond(&mut socket, "200 OK", page).await;
                return result;
            }
        }
    }
}

/// First line of the HTTP request (`GET /callback?code=… HTTP/1.1`).
async fn request_line(socket: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.contains(&b'\n') && buf.len() < 8192 {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let end = buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len());
    Ok(String::from_utf8_lossy(&buf[..end]).trim_end().to_owned())
}

async fn respond(socket: &mut TcpStream, status: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body.as_bytes()).await;
}

#[derive(Debug, PartialEq)]
enum Callback {
    /// Anything but `GET /callback`: answer 404 and keep listening.
    Other,
    Answer(Answer),
}

#[derive(Debug, Default, PartialEq)]
struct Answer {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn parse_request_line(line: &str) -> Callback {
    let mut parts = line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Other;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        return Callback::Other;
    }
    let mut answer = Answer::default();
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        match key {
            "code" => answer.code = Some(value.to_owned()),
            "state" => answer.state = Some(value.to_owned()),
            "error" => answer.error = Some(value.to_owned()),
            _ => {}
        }
    }
    Callback::Answer(answer)
}

fn check_answer(answer: Answer, state: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        answer.state.as_deref() == Some(state),
        "Antwort von Spotify mit falschem state verworfen"
    );
    if let Some(error) = answer.error {
        bail!("Anmeldung in Spotify abgebrochen ({error})");
    }
    answer
        .code
        .filter(|code| !code.is_empty())
        .context("Spotify hat keinen Code geliefert")
}

async fn exchange(
    http: &reqwest::Client,
    client_id: &str,
    code: &str,
    verifier: &str,
) -> anyhow::Result<Tokens> {
    let res = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .context("accounts.spotify.com nicht erreichbar")?;
    let ok = res.status().is_success();
    let body = body_json(res).await?;
    anyhow::ensure!(
        ok,
        "Spotify lehnt die Anmeldung ab ({}) – Client-ID prüfen",
        reason(&body)
    );
    tokens_from(&body, now()).context("keine Anmeldedaten in der Antwort")
}

/// A rejected refresh token (400/401) means the sign-in is gone ([`ReLogin`]);
/// other failures may be temporary, so the tokens are kept for the next try.
pub async fn refresh(
    http: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> anyhow::Result<Tokens> {
    let res = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await
        .context("accounts.spotify.com nicht erreichbar")?;
    let status = res.status().as_u16();
    if matches!(status, 400 | 401) {
        return Err(ReLogin.into());
    }
    let body = body_json(res).await?;
    anyhow::ensure!(
        (200..300).contains(&status),
        "Spotify-Anmeldung nicht erneuerbar (HTTP {status})"
    );
    tokens_from(&body, now())
        .map(|tokens| keep_refresh_token(tokens, refresh_token))
        .context("keine Anmeldedaten in der Antwort")
}

/// `(user id, display name)` of the signed-in account.
pub async fn profile(
    http: &reqwest::Client,
    access_token: &str,
) -> anyhow::Result<(String, String)> {
    let res = http
        .get(ME_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .context("api.spotify.com nicht erreichbar")?;
    let status = res.status().as_u16();
    let body = body_json(res).await?;
    anyhow::ensure!(
        (200..300).contains(&status),
        "Konto konnte nicht gelesen werden (HTTP {status})"
    );
    let id = body["id"]
        .as_str()
        .context("Antwort von Spotify ohne Benutzer-ID")?
        .to_owned();
    let name = body["display_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map_or_else(|| id.clone(), str::to_owned);
    Ok((id, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pkce_challenge_matches_rfc_7636_vector() {
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn callback_request_line() {
        assert_eq!(
            parse_request_line("GET /callback?code=abc&state=xyz HTTP/1.1"),
            Callback::Answer(Answer {
                code: Some("abc".into()),
                state: Some("xyz".into()),
                error: None,
            })
        );
        assert_eq!(
            parse_request_line("GET /callback?error=access_denied&state=xyz HTTP/1.1"),
            Callback::Answer(Answer {
                code: None,
                state: Some("xyz".into()),
                error: Some("access_denied".into()),
            })
        );
        assert_eq!(
            parse_request_line("GET /favicon.ico HTTP/1.1"),
            Callback::Other
        );
        assert_eq!(
            parse_request_line("POST /callback?code=abc HTTP/1.1"),
            Callback::Other
        );
        assert_eq!(parse_request_line(""), Callback::Other);
    }

    #[test]
    fn callback_state_must_match() {
        let answer = |state: &str| Answer {
            code: Some("abc".into()),
            state: Some(state.into()),
            error: None,
        };
        assert_eq!(check_answer(answer("xyz"), "xyz").unwrap(), "abc");
        assert!(check_answer(answer("other"), "xyz").is_err());
        let denied = Answer {
            code: None,
            state: Some("xyz".into()),
            error: Some("access_denied".into()),
        };
        assert!(check_answer(denied, "xyz").is_err());
    }

    #[test]
    fn refreshes_when_less_than_a_minute_is_left() {
        assert!(needs_refresh(1059, 1000));
        assert!(!needs_refresh(1060, 1000));
    }

    #[test]
    fn token_answer_gets_expiry_and_keeps_refresh_token() {
        let body = json!({ "access_token": "AT", "refresh_token": "RT", "expires_in": 3600 });
        let tokens = tokens_from(&body, 100).unwrap();
        assert_eq!(tokens.expires_at, 3700);
        let kept = keep_refresh_token(
            tokens_from(&json!({ "access_token": "AT2", "expires_in": 3600 }), 0).unwrap(),
            "RT-old",
        );
        assert_eq!(kept.refresh_token, "RT-old");
        assert_eq!(kept.access_token, "AT2");
        assert!(tokens_from(&json!({}), 0).is_none());
    }
}

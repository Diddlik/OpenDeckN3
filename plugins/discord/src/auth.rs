//! Connecting and signing in: IPC handshake → AUTHORIZE (popup in Discord)
//! → OAuth2 token exchange → AUTHENTICATE. Saved tokens skip the popup.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::ipc::{IpcEvent, Rpc};

/// Always needed (voice control, notifications).
const BASE_SCOPES: [&str; 5] = [
    "rpc",
    "identify",
    "rpc.voice.read",
    "rpc.voice.write",
    "rpc.notifications.read",
];
/// Camera and screen share; Discord may refuse them for unapproved apps.
const EXTRA_SCOPES: [&str; 4] = [
    "rpc.video.read",
    "rpc.video.write",
    "rpc.screenshare.read",
    "rpc.screenshare.write",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

impl Credentials {
    pub fn complete(&self) -> bool {
        !self.client_id.is_empty() && !self.client_secret.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

pub struct Session {
    pub rpc: Rpc,
    pub tokens: Tokens,
    pub user: String,
}

/// Saved tokens are missing or invalid and the caller did not allow the
/// authorization popup.
#[derive(Debug)]
pub struct NeedsAuthorize;

impl std::fmt::Display for NeedsAuthorize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Noch nicht mit Discord verbunden – „Mit Discord verbinden“ klicken")
    }
}

impl std::error::Error for NeedsAuthorize {}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

pub async fn connect(
    creds: &Credentials,
    tokens: Option<Tokens>,
    allow_authorize: bool,
    events: mpsc::UnboundedSender<IpcEvent>,
) -> anyhow::Result<Session> {
    let rpc = Rpc::connect(&creds.client_id, events).await?;
    sign_in(rpc, creds, tokens, allow_authorize).await
}

/// Everything after the handshake (separate for tests).
pub async fn sign_in(
    rpc: Rpc,
    creds: &Credentials,
    tokens: Option<Tokens>,
    allow_authorize: bool,
) -> anyhow::Result<Session> {
    let http = http()?;
    if let Some(mut tokens) = tokens.filter(|t| !t.access_token.is_empty()) {
        // Refresh a day early; refresh failures fall through to AUTHORIZE.
        if tokens.expires_at < now() + 24 * 3600 && !tokens.refresh_token.is_empty() {
            match token_request(
                &http,
                creds,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &tokens.refresh_token),
                ],
            )
            .await
            {
                Ok(fresh) => tokens = fresh,
                Err(err) => eprintln!("discord: token refresh failed: {err:#}"),
            }
        }
        match authenticate(&rpc, &tokens).await {
            Ok(user) => return Ok(Session { rpc, tokens, user }),
            Err(err) => eprintln!("discord: saved token rejected: {err:#}"),
        }
    }
    if !allow_authorize {
        rpc.close();
        return Err(NeedsAuthorize.into());
    }

    let code = match authorize(&rpc, &creds.client_id, true).await {
        Ok(code) => code,
        Err(err) if err.to_string().to_lowercase().contains("scope") => {
            authorize(&rpc, &creds.client_id, false).await?
        }
        Err(err) => return Err(err),
    };
    let tokens = token_request(
        &http,
        creds,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &creds.redirect_uri),
        ],
    )
    .await?;
    let user = authenticate(&rpc, &tokens).await?;
    Ok(Session { rpc, tokens, user })
}

async fn authorize(rpc: &Rpc, client_id: &str, extra: bool) -> anyhow::Result<String> {
    let mut scopes: Vec<&str> = BASE_SCOPES.to_vec();
    if extra {
        scopes.extend(EXTRA_SCOPES);
    }
    let data = rpc
        .call_with_timeout(
            "AUTHORIZE",
            json!({ "client_id": client_id, "scopes": scopes }),
            None,
            // The user has to confirm the popup in Discord.
            Duration::from_secs(300),
        )
        .await
        .map_err(|err| {
            let text = err.to_string();
            if text.contains("access_denied") || text.contains("cancel") {
                anyhow!("Verbindung in Discord abgelehnt")
            } else if text.to_lowercase().contains("tester") || text.contains("whitelist") {
                anyhow!(
                    "Discord erlaubt die Anwendung nicht: eigenes Konto unter „App Testers“ eintragen ({text})"
                )
            } else {
                err.context("Autorisierung fehlgeschlagen")
            }
        })?;
    data["code"]
        .as_str()
        .map(str::to_owned)
        .context("Discord lieferte keinen Autorisierungscode")
}

async fn authenticate(rpc: &Rpc, tokens: &Tokens) -> anyhow::Result<String> {
    let data = rpc
        .call(
            "AUTHENTICATE",
            json!({ "access_token": tokens.access_token }),
        )
        .await?;
    let user = &data["user"];
    Ok(user["global_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| user["username"].as_str())
        .unwrap_or("?")
        .to_owned())
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("OpenDeckN3-Discord/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()?)
}

/// `OPENDECKN3_DISCORD_API` overrides the API base (tests).
fn api_base() -> String {
    std::env::var("OPENDECKN3_DISCORD_API").unwrap_or_else(|_| "https://discord.com/api/v10".into())
}

async fn token_request(
    http: &reqwest::Client,
    creds: &Credentials,
    params: &[(&str, &str)],
) -> anyhow::Result<Tokens> {
    let mut form: Vec<(&str, &str)> = vec![
        ("client_id", &creds.client_id),
        ("client_secret", &creds.client_secret),
    ];
    form.extend_from_slice(params);
    let res = http
        .post(format!("{}/oauth2/token", api_base()))
        .form(&form)
        .send()
        .await
        .context("discord.com nicht erreichbar")?;
    let status = res.status();
    let body: Value = serde_json::from_slice(&res.bytes().await?).unwrap_or_default();
    if !status.is_success() {
        let reason = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("unbekannt");
        if body["error"] == "invalid_client" {
            bail!("Client-Secret ist falsch ({reason})");
        }
        if reason.contains("redirect_uri") {
            bail!("Redirect-URI stimmt nicht mit der Anwendung überein ({reason})");
        }
        bail!("Token-Anfrage abgelehnt: {reason} (HTTP {status})");
    }
    Ok(Tokens {
        access_token: body["access_token"]
            .as_str()
            .context("kein access_token")?
            .to_owned(),
        refresh_token: body["refresh_token"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        expires_at: now() + body["expires_in"].as_u64().unwrap_or(604_800),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::tests::fake_discord;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Tiny HTTP server answering the token endpoint; records request bodies.
    async fn fake_token_server(log: Arc<Mutex<Vec<String>>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                log.lock().unwrap().push(req.clone());
                let body = r#"{"access_token":"AT","refresh_token":"RT","expires_in":604800}"#;
                let res = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(res.as_bytes()).await.unwrap();
            }
        });
        format!("http://{addr}")
    }

    fn creds() -> Credentials {
        Credentials {
            client_id: "42".into(),
            client_secret: "s3cret".into(),
            redirect_uri: "http://localhost".into(),
        }
    }

    #[tokio::test]
    async fn full_authorization_flow() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let base = fake_token_server(log.clone()).await;
        // SAFETY: tests in this module run the token server; nothing else reads it.
        unsafe { std::env::set_var("OPENDECKN3_DISCORD_API", &base) };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let stream = fake_discord(move |f| {
            seen2
                .lock()
                .unwrap()
                .push(f["cmd"].as_str().unwrap().to_owned());
            let data = match f["cmd"].as_str().unwrap() {
                "AUTHORIZE" => {
                    assert!(
                        f["args"]["scopes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|s| s == "rpc.voice.write")
                    );
                    json!({ "code": "CODE1" })
                }
                "AUTHENTICATE" => {
                    assert_eq!(f["args"]["access_token"], "AT");
                    json!({ "user": { "username": "max", "global_name": "Max" } })
                }
                _ => json!({}),
            };
            vec![json!({ "cmd": f["cmd"], "nonce": f["nonce"], "data": data })]
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let rpc = Rpc::handshake(stream, "42", tx).await.unwrap();
        let session = sign_in(rpc, &creds(), None, true).await.unwrap();
        assert_eq!(session.user, "Max");
        assert_eq!(session.tokens.refresh_token, "RT");
        assert!(session.tokens.expires_at > now());
        assert_eq!(*seen.lock().unwrap(), ["AUTHORIZE", "AUTHENTICATE"]);
        let req = log.lock().unwrap().join("\n");
        assert!(
            req.contains("grant_type=authorization_code") && req.contains("code=CODE1"),
            "{req}"
        );
        assert!(req.contains("redirect_uri=http%3A%2F%2Flocalhost"), "{req}");

        // Saved, still valid token: no popup.
        let stream = fake_discord(|f| {
            assert_ne!(f["cmd"], "AUTHORIZE");
            vec![json!({ "cmd": f["cmd"], "nonce": f["nonce"],
                "data": { "user": { "username": "max" } } })]
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let rpc = Rpc::handshake(stream, "42", tx).await.unwrap();
        let tokens = Tokens {
            access_token: "AT".into(),
            refresh_token: "RT".into(),
            expires_at: now() + 5 * 86400,
        };
        let session = sign_in(rpc, &creds(), Some(tokens), false).await.unwrap();
        assert_eq!(session.user, "max");
    }

    #[tokio::test]
    async fn without_token_and_popup_needs_authorize() {
        let stream =
            fake_discord(|f| vec![json!({ "cmd": f["cmd"], "nonce": f["nonce"], "data": {} })]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let rpc = Rpc::handshake(stream, "42", tx).await.unwrap();
        let err = sign_in(rpc, &creds(), None, false).await.err().unwrap();
        assert!(err.is::<NeedsAuthorize>());
    }
}

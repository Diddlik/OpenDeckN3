//! Sign-in with Google's device flow (“TVs and Limited Input devices”) and
//! token refresh. The plugin keeps the tokens in its global settings.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde_json::Value;

const SCOPE: &str = "https://www.googleapis.com/auth/youtube";
const DEVICE_URL: &str = "https://oauth2.googleapis.com/device/code";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Refresh the access token when less than this many seconds are left.
const REFRESH_MARGIN: u64 = 60;

#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

#[derive(Clone)]
pub struct Device {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Outcome of one token request.
pub enum Step {
    Done(Tokens),
    /// The user has not confirmed the code yet.
    Wait,
    SlowDown,
    Expired,
    Denied,
    Failed(String),
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

/// Google's refresh answer has no `refresh_token`; the old one stays valid.
pub fn keep_refresh_token(mut tokens: Tokens, old: &str) -> Tokens {
    if tokens.refresh_token.is_empty() {
        tokens.refresh_token = old.to_owned();
    }
    tokens
}

fn field(body: &Value, key: &str) -> anyhow::Result<String> {
    body[key]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("Antwort von Google ohne „{key}“"))
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

/// Classifies a token response. Google reports the device flow states as
/// `error` values in the JSON body.
pub fn token_step(ok: bool, body: &Value, now: u64) -> Step {
    if ok {
        return tokens_from(body, now).map_or_else(
            || Step::Failed("keine Anmeldedaten in der Antwort".into()),
            Step::Done,
        );
    }
    match body["error"].as_str().unwrap_or_default() {
        "authorization_pending" => Step::Wait,
        "slow_down" => Step::SlowDown,
        "access_denied" => Step::Denied,
        "expired_token" => Step::Expired,
        other => Step::Failed(other.to_owned()),
    }
}

async fn body_json(res: reqwest::Response) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&res.bytes().await?).unwrap_or_default())
}

pub async fn device_code(http: &reqwest::Client, client_id: &str) -> anyhow::Result<Device> {
    let res = http
        .post(DEVICE_URL)
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await
        .context("oauth2.googleapis.com nicht erreichbar")?;
    let ok = res.status().is_success();
    let body = body_json(res).await?;
    anyhow::ensure!(
        ok,
        "Google lehnt die Anmeldung ab ({}) – Client-ID prüfen",
        body["error"].as_str().unwrap_or_default()
    );
    Ok(Device {
        device_code: field(&body, "device_code")?,
        user_code: field(&body, "user_code")?,
        verification_url: field(&body, "verification_url")?,
        expires_in: body["expires_in"].as_u64().unwrap_or(1800),
        interval: body["interval"].as_u64().unwrap_or(5),
    })
}

/// Polls the token endpoint until the user confirmed, denied, or the code expired.
pub async fn poll(
    http: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    device: &Device,
) -> anyhow::Result<Tokens> {
    let deadline = now() + device.expires_in;
    let mut interval = device.interval;
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        anyhow::ensure!(
            now() < deadline,
            "Anmeldecode abgelaufen – erneut „Anmelden“ klicken"
        );
        let res = http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("device_code", device.device_code.as_str()),
                ("grant_type", DEVICE_GRANT),
            ])
            .send()
            .await
            .context("oauth2.googleapis.com nicht erreichbar")?;
        let ok = res.status().is_success();
        match token_step(ok, &body_json(res).await?, now()) {
            Step::Done(tokens) => return Ok(tokens),
            Step::Wait => {}
            Step::SlowDown => interval += 5,
            Step::Expired => bail!("Anmeldecode abgelaufen – erneut „Anmelden“ klicken"),
            Step::Denied => bail!("Anmeldung im Browser abgelehnt"),
            Step::Failed(reason) => bail!("Google-Anmeldung fehlgeschlagen: {reason}"),
        }
    }
}

/// A rejected refresh token means the sign-in is gone ([`ReLogin`]); a
/// network error does not, so the tokens are kept for the next try.
pub async fn refresh(
    http: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> anyhow::Result<Tokens> {
    let res = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .context("oauth2.googleapis.com nicht erreichbar")?;
    let ok = res.status().is_success();
    match token_step(ok, &body_json(res).await?, now()) {
        Step::Done(tokens) => Ok(keep_refresh_token(tokens, refresh_token)),
        _ => Err(ReLogin.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refreshes_when_less_than_a_minute_is_left() {
        assert!(needs_refresh(1059, 1000));
        assert!(!needs_refresh(1060, 1000));
    }

    #[test]
    fn device_flow_answers() {
        let pending = json!({ "error": "authorization_pending" });
        assert!(matches!(token_step(false, &pending, 0), Step::Wait));
        assert!(matches!(
            token_step(false, &json!({ "error": "slow_down" }), 0),
            Step::SlowDown
        ));
        assert!(matches!(
            token_step(false, &json!({ "error": "expired_token" }), 0),
            Step::Expired
        ));
        assert!(matches!(
            token_step(false, &json!({ "error": "access_denied" }), 0),
            Step::Denied
        ));
        assert!(matches!(
            token_step(false, &json!({ "error": "invalid_client" }), 0),
            Step::Failed(reason) if reason == "invalid_client"
        ));
    }

    #[test]
    fn token_answer_gets_expiry() {
        let body = json!({ "access_token": "AT", "refresh_token": "RT", "expires_in": 3599 });
        assert!(matches!(
            token_step(true, &body, 100),
            Step::Done(t) if t.expires_at == 3699 && t.refresh_token == "RT"
        ));
        assert!(matches!(token_step(true, &json!({}), 0), Step::Failed(_)));
    }

    #[test]
    fn refresh_without_new_refresh_token_keeps_the_old_one() {
        let tokens = Tokens {
            access_token: "AT2".into(),
            refresh_token: String::new(),
            expires_at: 0,
        };
        let kept = keep_refresh_token(tokens, "RT-old");
        assert_eq!(kept.refresh_token, "RT-old");
        assert_eq!(kept.access_token, "AT2");
    }
}

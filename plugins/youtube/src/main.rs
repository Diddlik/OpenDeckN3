#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 YouTube plugin: live status, concurrent viewers and chat
//! messages over the YouTube Data API v3 (see `docs/PLUGIN_YOUTUBE.md`).

mod auth;

use std::time::{Duration, Instant};

use anyhow::Context;
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const API: &str = "https://www.googleapis.com/youtube/v3";
const STUDIO_URL: &str = "https://studio.youtube.com";
const NEEDS_CREDENTIALS: &str =
    "Client-ID und Client-Secret eintragen, siehe Anleitung in der Dokumentation";
/// Live status and viewer count are polled at most this often, one request for all keys.
const POLL: Duration = Duration::from_secs(60);
/// Polling stops this long after the daily quota ran out.
const QUOTA_PAUSE: Duration = Duration::from_secs(60 * 60);
/// Global settings written by a sign-in; cleared on sign-out.
const ACCOUNT_KEYS: [&str; 3] = ["accessToken", "refreshToken", "expiresAt"];

/// Signed-in user with the tokens to call the API with.
struct Session {
    client_id: String,
    client_secret: String,
    tokens: auth::Tokens,
}

/// One API request: method, path below `/youtube/v3` and optional JSON body.
struct Req {
    method: reqwest::Method,
    path: String,
    body: Option<Value>,
}

impl Req {
    fn get(path: impl Into<String>) -> Self {
        Self {
            method: reqwest::Method::GET,
            path: path.into(),
            body: None,
        }
    }

    fn post(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: reqwest::Method::POST,
            path: path.into(),
            body: Some(body),
        }
    }
}

/// The authorized user's active live broadcast.
struct Broadcast {
    id: String,
    chat_id: String,
}

/// The daily API quota is used up.
#[derive(Debug)]
struct Quota;

impl std::fmt::Display for Quota {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YouTube-Kontingent erschöpft")
    }
}

impl std::error::Error for Quota {}

enum Msg {
    Code(u64, auth::Device),
    Signed(u64, anyhow::Result<auth::Tokens>),
    Tokens(u64, auth::Tokens),
    Stream(u64, anyhow::Result<Stream>),
    /// Chat message sent for the key with this context.
    Sent(u64, String, anyhow::Result<()>),
}

/// Live state as last polled.
#[derive(Clone, Copy)]
enum Stream {
    Unknown,
    Offline,
    Live(u64),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    http: reqwest::Client,
    contexts: Contexts,
    global: Map<String, Value>,
    /// Bumped when an account changes; results of older requests are dropped.
    generation: u64,
    /// The first global settings message only loads values and must not
    /// start a sign-in.
    loaded: bool,
    stream: Stream,
    last_poll: Option<Instant>,
    paused_until: Option<Instant>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>, http: reqwest::Client) -> Self {
        Self {
            host,
            tx,
            http,
            contexts: Contexts::default(),
            global: Map::new(),
            generation: 0,
            loaded: false,
            stream: Stream::Unknown,
            last_poll: None,
            paused_until: None,
        }
    }

    fn save(&self) {
        self.host.save_global(&self.global);
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.global.insert("status".into(), json!(status.into()));
        self.save();
    }

    fn open_url(&self, url: &str) {
        self.host
            .send(json!({ "event": "openUrl", "payload": { "url": url } }));
    }

    fn session(&self) -> Option<Session> {
        let access_token = global_str(&self.global, "accessToken");
        (!access_token.is_empty()).then(|| Session {
            client_id: global_str(&self.global, "clientId"),
            client_secret: global_str(&self.global, "clientSecret"),
            tokens: auth::Tokens {
                access_token,
                refresh_token: global_str(&self.global, "refreshToken"),
                expires_at: self
                    .global
                    .get("expiresAt")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            },
        })
    }

    fn store_tokens(&mut self, tokens: &auth::Tokens) {
        self.global
            .insert("accessToken".into(), json!(tokens.access_token));
        self.global
            .insert("refreshToken".into(), json!(tokens.refresh_token));
        self.global
            .insert("expiresAt".into(), json!(tokens.expires_at));
    }

    fn has_credentials(&self) -> bool {
        !global_str(&self.global, "clientId").is_empty()
            && !global_str(&self.global, "clientSecret").is_empty()
    }

    fn refresh_status(&mut self) {
        let status = if self.session().is_some() {
            "Verbunden"
        } else if self.has_credentials() {
            "Nicht angemeldet – „Anmelden“ klicken"
        } else {
            NEEDS_CREDENTIALS
        };
        self.set_status(status);
    }

    /// Drops the account and everything read with it. The quota pause stays,
    /// because the quota belongs to the Google project.
    fn forget_account(&mut self) {
        self.generation += 1;
        for key in ACCOUNT_KEYS {
            self.global.remove(key);
        }
        self.stream = Stream::Unknown;
        self.last_poll = None;
        self.refresh_all();
    }

    fn on_global(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        if !std::mem::replace(&mut self.loaded, true) {
            self.refresh_status();
            return;
        }
        let changed = |key: &str| previous.get(key) != self.global.get(key);
        if changed("signOut") {
            self.forget_account();
            self.set_status("Abgemeldet");
        } else if changed("connect") {
            self.start_sign_in();
        } else if changed("clientId") || changed("clientSecret") {
            self.forget_account();
            self.refresh_status();
        }
    }

    fn start_sign_in(&mut self) {
        self.generation += 1;
        if !self.has_credentials() {
            self.set_status(NEEDS_CREDENTIALS);
            return;
        }
        self.set_status("Anmeldung läuft …");
        let client_id = global_str(&self.global, "clientId");
        let client_secret = global_str(&self.global, "clientSecret");
        let (http, tx, generation) = (self.http.clone(), self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let result = sign_in(&http, &tx, generation, &client_id, &client_secret).await;
            tx.send(Msg::Signed(generation, result)).ok();
        });
    }

    fn on_code(&mut self, generation: u64, device: auth::Device) {
        if generation != self.generation {
            return;
        }
        self.set_status(format!("Code eingeben: {}", device.user_code));
        self.open_url(&device.verification_url);
    }

    fn on_signed(&mut self, generation: u64, result: anyhow::Result<auth::Tokens>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(tokens) => {
                self.store_tokens(&tokens);
                self.set_status("Verbunden");
                self.poll_stream();
            }
            Err(err) => self.set_status(format!("Anmeldung fehlgeschlagen: {err:#}")),
        }
    }

    fn on_stream(&mut self, generation: u64, result: anyhow::Result<Stream>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(stream) => {
                self.stream = stream;
                self.refresh_all();
            }
            Err(err) => self.on_failure(&err),
        }
    }

    fn on_sent(&mut self, generation: u64, context: String, result: anyhow::Result<()>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(()) => self.host.ok(&context),
            Err(err) => {
                self.host.alert(&context);
                self.on_failure(&err);
            }
        }
    }

    /// Handles errors that change the plugin state; all errors go to the log.
    fn on_failure(&mut self, err: &anyhow::Error) {
        if err.is::<auth::ReLogin>() {
            self.forget_account();
            self.set_status("Bitte neu anmelden – „Anmelden“ klicken");
        } else if err.is::<Quota>() {
            self.paused_until = Some(Instant::now() + QUOTA_PAUSE);
            self.set_status("YouTube-Kontingent erschöpft – Abfragen pausiert für 1 Stunde");
        }
        self.host.log(format!("{err:#}"));
    }

    /// Fetches the live state if a status or viewer key is shown and the last poll is old enough.
    fn poll_stream(&mut self) {
        let due = self.last_poll.is_none_or(|at| at.elapsed() >= POLL);
        let paused = self
            .paused_until
            .is_some_and(|until| Instant::now() < until);
        let shown = self.contexts.any("status") || self.contexts.any("viewers");
        let Some(session) = self.session().filter(|_| due && !paused && shown) else {
            return;
        };
        self.last_poll = Some(Instant::now());
        let (http, tx, generation) = (self.http.clone(), self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let mut session = session;
            let result = stream_state(&http, &tx, generation, &mut session).await;
            tx.send(Msg::Stream(generation, result)).ok();
        });
    }

    fn on_host(&mut self, msg: Value) {
        if self.contexts.apply(&msg).is_some() {
            if msg["event"] == "willAppear" {
                self.poll_stream();
            }
            self.refresh_all();
            return;
        }
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "keyDown" => self.press(msg["context"].as_str().unwrap_or_default()),
            _ => {}
        }
    }

    fn press(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        if inst.kind() == "status" {
            self.open_url(STUDIO_URL);
            return;
        }
        if inst.kind() != "chat" {
            return;
        }
        let Some(session) = self.session() else {
            self.host.alert(context);
            self.host
                .log("Nicht angemeldet – Plugins → YouTube → Einstellungen → Anmelden");
            return;
        };
        let message = inst.text("message");
        if message.is_empty() {
            self.host.alert(context);
            self.host
                .log("Keine Nachricht eingetragen – Einstellung „Nachricht“ ausfüllen");
            return;
        }
        let (http, tx, generation) = (self.http.clone(), self.tx.clone(), self.generation);
        let context = context.to_owned();
        tokio::spawn(async move {
            let mut session = session;
            let result = send_message(&http, &tx, generation, &mut session, &message).await;
            tx.send(Msg::Sent(generation, context, result)).ok();
        });
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        match (inst.kind(), self.stream) {
            ("status", Stream::Unknown) => (0, String::new()),
            ("status", Stream::Offline) => (0, "offline".into()),
            ("status", Stream::Live(_)) => (1, "LIVE".into()),
            ("viewers", Stream::Unknown) => (0, String::new()),
            ("viewers", Stream::Offline) => (0, "offline".into()),
            ("viewers", Stream::Live(count)) => (0, viewer_title(count)),
            _ => (0, String::new()),
        }
    }

    fn refresh_all(&mut self) {
        for context in self.contexts.ids() {
            if let Some(inst) = self.contexts.get(&context) {
                let (state, title) = self.appearance(inst);
                self.contexts.show(&self.host, &context, state, &title);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Code(generation, device) => self.on_code(generation, device),
            Msg::Signed(generation, result) => self.on_signed(generation, result),
            Msg::Tokens(generation, tokens) => {
                if generation == self.generation {
                    self.store_tokens(&tokens);
                    self.save();
                }
            }
            Msg::Stream(generation, result) => self.on_stream(generation, result),
            Msg::Sent(generation, context, result) => self.on_sent(generation, context, result),
        }
    }
}

async fn sign_in(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    client_id: &str,
    client_secret: &str,
) -> anyhow::Result<auth::Tokens> {
    let device = auth::device_code(http, client_id).await?;
    tx.send(Msg::Code(generation, device.clone())).ok();
    auth::poll(http, client_id, client_secret, &device).await
}

/// Live status and, if a broadcast is running, its viewer count.
async fn stream_state(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    session: &mut Session,
) -> anyhow::Result<Stream> {
    let body = api(http, tx, generation, session, &live_request()).await?;
    let Some(broadcast) = active_broadcast(&body) else {
        return Ok(Stream::Offline);
    };
    let path = format!("/videos?part=liveStreamingDetails&id={}", broadcast.id);
    let body = api(http, tx, generation, session, &Req::get(path)).await?;
    Ok(Stream::Live(concurrent_viewers(&body)))
}

async fn send_message(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    session: &mut Session,
    message: &str,
) -> anyhow::Result<()> {
    let body = api(http, tx, generation, session, &live_request()).await?;
    let broadcast = active_broadcast(&body).context("Keine aktive Übertragung")?;
    let req = Req::post(
        "/liveChat/messages?part=snippet",
        json!({
            "snippet": {
                "liveChatId": broadcast.chat_id,
                "type": "textMessageEvent",
                "textMessageDetails": { "messageText": message },
            }
        }),
    );
    api(http, tx, generation, session, &req).await?;
    Ok(())
}

fn live_request() -> Req {
    Req::get("/liveBroadcasts?part=snippet,status&broadcastStatus=active&broadcastType=all")
}

/// Runs one API request. The token is refreshed before it expires and once
/// more after a 401; new tokens are handed back to the plugin.
async fn api(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    session: &mut Session,
    req: &Req,
) -> anyhow::Result<Value> {
    if auth::needs_refresh(session.tokens.expires_at, auth::now()) {
        refresh(http, tx, generation, session).await?;
    }
    let (mut code, mut body) = send(http, session, req).await?;
    if code == 401 {
        refresh(http, tx, generation, session).await?;
        (code, body) = send(http, session, req).await?;
    }
    if quota_exceeded(code, &body) {
        return Err(Quota.into());
    }
    anyhow::ensure!((200..300).contains(&code), "{}", api_error(code, &body));
    Ok(body)
}

async fn refresh(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    session: &mut Session,
) -> anyhow::Result<()> {
    // Requests started with the same old token (e.g. several keys after the PC
    // wakes up) must share one refresh.
    static LAST: tokio::sync::Mutex<Option<(String, auth::Tokens)>> =
        tokio::sync::Mutex::const_new(None);
    let mut last = LAST.lock().await;
    if let Some((old, tokens)) = last.as_ref()
        && *old == session.tokens.refresh_token
    {
        session.tokens = tokens.clone();
        return Ok(());
    }
    let tokens = auth::refresh(
        http,
        &session.client_id,
        &session.client_secret,
        &session.tokens.refresh_token,
    )
    .await?;
    *last = Some((session.tokens.refresh_token.clone(), tokens.clone()));
    tx.send(Msg::Tokens(generation, tokens.clone())).ok();
    session.tokens = tokens;
    Ok(())
}

async fn send(
    http: &reqwest::Client,
    session: &Session,
    req: &Req,
) -> anyhow::Result<(u16, Value)> {
    let mut call = http
        .request(req.method.clone(), format!("{API}{}", req.path))
        .bearer_auth(&session.tokens.access_token);
    if let Some(body) = &req.body {
        call = call.json(body);
    }
    let res = call.send().await.context("YouTube nicht erreichbar")?;
    let code = res.status().as_u16();
    let body = serde_json::from_slice(&res.bytes().await?).unwrap_or_default();
    Ok((code, body))
}

/// Google answers an exhausted quota with HTTP 403 and reason `quotaExceeded`.
fn quota_exceeded(code: u16, body: &Value) -> bool {
    code == 403 && body["error"]["errors"][0]["reason"].as_str() == Some("quotaExceeded")
}

fn api_error(code: u16, body: &Value) -> String {
    match body["error"]["message"].as_str().filter(|m| !m.is_empty()) {
        Some(message) => format!("YouTube antwortet mit HTTP {code}: {message}"),
        None => format!("YouTube antwortet mit HTTP {code}"),
    }
}

/// The first broadcast of a `liveBroadcasts` answer that has a live chat.
fn active_broadcast(body: &Value) -> Option<Broadcast> {
    body["items"].as_array()?.iter().find_map(|item| {
        Some(Broadcast {
            id: item["id"].as_str()?.to_owned(),
            chat_id: item["snippet"]["liveChatId"].as_str()?.to_owned(),
        })
    })
}

/// YouTube sends counts as strings; a missing count counts as no viewers.
fn concurrent_viewers(body: &Value) -> u64 {
    body["items"][0]["liveStreamingDetails"]["concurrentViewers"]
        .as_str()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

/// `987`, `1,2 Tsd.`, `15 Tsd.`
fn viewer_title(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    let tenths = (count + 50) / 100;
    if tenths.is_multiple_of(10) {
        format!("{} Tsd.", tenths / 10)
    } else {
        format!("{},{} Tsd.", tenths / 10, tenths % 10)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let http = reqwest::Client::builder()
        .user_agent(concat!("OpenDeckN3-YouTube/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut plugin = Plugin::new(host, tx, http);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.handle(msg),
            _ = tick.tick() => plugin.poll_stream(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_count_is_abbreviated_from_a_thousand() {
        assert_eq!(viewer_title(0), "0");
        assert_eq!(viewer_title(999), "999");
        assert_eq!(viewer_title(1000), "1 Tsd.");
        assert_eq!(viewer_title(1234), "1,2 Tsd.");
        assert_eq!(viewer_title(12_345), "12,3 Tsd.");
    }

    #[test]
    fn picks_the_first_broadcast_with_a_live_chat() {
        let body = json!({
            "items": [
                { "id": "no-chat", "snippet": {} },
                { "id": "vid1", "snippet": { "liveChatId": "chat1" } },
                { "id": "vid2", "snippet": { "liveChatId": "chat2" } },
            ]
        });
        let broadcast = active_broadcast(&body).expect("broadcast");
        assert_eq!(broadcast.id, "vid1");
        assert_eq!(broadcast.chat_id, "chat1");
        assert!(active_broadcast(&json!({ "items": [] })).is_none());
        assert!(active_broadcast(&json!({})).is_none());
    }

    #[test]
    fn viewers_are_read_from_string_count() {
        let body = json!({
            "items": [{ "liveStreamingDetails": { "concurrentViewers": "1234" } }]
        });
        assert_eq!(concurrent_viewers(&body), 1234);
        assert_eq!(concurrent_viewers(&json!({ "items": [{}] })), 0);
        assert_eq!(concurrent_viewers(&json!({})), 0);
    }

    #[test]
    fn quota_error_is_detected() {
        let quota = json!({
            "error": { "errors": [{ "reason": "quotaExceeded" }], "message": "x" }
        });
        assert!(quota_exceeded(403, &quota));
        assert!(!quota_exceeded(500, &quota));
        let other = json!({ "error": { "errors": [{ "reason": "forbidden" }] } });
        assert!(!quota_exceeded(403, &other));
        assert!(!quota_exceeded(403, &json!({})));
    }
}

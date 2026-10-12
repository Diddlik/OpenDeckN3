#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 Twitch plugin: stream markers, clips, chat modes, ads and the
//! live viewer count over the Twitch Helix API (see `docs/PLUGIN_TWITCH.md`).

mod auth;

use std::time::{Duration, Instant};

use anyhow::Context;
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const HELIX: &str = "https://api.twitch.tv/helix";
/// The viewer count is polled at most this often, one request for all keys.
const VIEWER_POLL: Duration = Duration::from_secs(60);
/// Global settings written by a sign-in; cleared on sign-out.
const ACCOUNT_KEYS: [&str; 5] = [
    "accessToken",
    "refreshToken",
    "expiresAt",
    "userId",
    "login",
];

/// Signed-in user with the tokens to call the API with.
#[derive(Clone)]
struct Session {
    client_id: String,
    user_id: String,
    tokens: auth::Tokens,
}

/// Result of a finished sign-in.
struct Account {
    user_id: String,
    login: String,
    tokens: auth::Tokens,
}

/// One Helix request: method, path below `/helix` and optional JSON body.
struct Req {
    method: reqwest::Method,
    path: String,
    body: Option<Value>,
}

/// What a finished API request means for the plugin.
enum Reply {
    /// Marker or ad: flash the key.
    Done(String),
    /// Clip created: flash the key, open the editor link when asked.
    Clip(String, bool),
    /// Chat mode switched to the given state.
    Chat(String, String, bool),
    /// Chat settings read.
    ChatRead,
    /// Stream lookup for the viewer count.
    Viewers,
}

impl Reply {
    fn context(&self) -> Option<&str> {
        match self {
            Reply::Done(context) | Reply::Clip(context, _) | Reply::Chat(context, _, _) => {
                Some(context)
            }
            Reply::ChatRead | Reply::Viewers => None,
        }
    }
}

enum Msg {
    Code(u64, auth::Device),
    Signed(u64, anyhow::Result<Account>),
    Tokens(u64, auth::Tokens),
    Api(u64, Reply, anyhow::Result<Value>),
}

/// Viewer count of the own channel as last polled.
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
    /// Chat settings of the channel (`Null` until read).
    chat: Value,
    stream: Stream,
    last_poll: Option<Instant>,
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
            chat: Value::Null,
            stream: Stream::Unknown,
            last_poll: None,
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
        let user_id = global_str(&self.global, "userId");
        (!access_token.is_empty() && !user_id.is_empty()).then(|| Session {
            client_id: global_str(&self.global, "clientId"),
            user_id,
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

    fn refresh_status(&mut self) {
        let status = if self.session().is_some() {
            format!("Verbunden als {}", global_str(&self.global, "login"))
        } else if global_str(&self.global, "clientId").is_empty() {
            "Client-ID eintragen, siehe Anleitung in der Dokumentation".to_owned()
        } else {
            "Nicht angemeldet – „Anmelden“ klicken".to_owned()
        };
        self.set_status(status);
    }

    /// Drops the account and everything read with it.
    fn forget_account(&mut self) {
        self.generation += 1;
        for key in ACCOUNT_KEYS {
            self.global.remove(key);
        }
        self.chat = Value::Null;
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
        } else if changed("clientId") {
            self.forget_account();
            self.refresh_status();
        }
    }

    fn start_sign_in(&mut self) {
        self.generation += 1;
        let client_id = global_str(&self.global, "clientId");
        if client_id.is_empty() {
            self.set_status("Client-ID eintragen, siehe Anleitung in der Dokumentation");
            return;
        }
        self.set_status("Anmeldung läuft …");
        let (http, tx, generation) = (self.http.clone(), self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let result = sign_in(&http, &tx, generation, &client_id).await;
            tx.send(Msg::Signed(generation, result)).ok();
        });
    }

    fn on_code(&mut self, generation: u64, device: auth::Device) {
        if generation != self.generation {
            return;
        }
        self.set_status(format!("Code eingeben: {}", device.user_code));
        self.open_url(&device.verification_uri);
    }

    fn on_signed(&mut self, generation: u64, result: anyhow::Result<Account>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(account) => {
                self.store_tokens(&account.tokens);
                self.global.insert("userId".into(), json!(account.user_id));
                self.global.insert("login".into(), json!(account.login));
                self.set_status(format!("Verbunden als {}", account.login));
                self.read_chat();
                self.poll_viewers();
            }
            Err(err) => self.set_status(format!("Anmeldung fehlgeschlagen: {err:#}")),
        }
    }

    fn on_api(&mut self, generation: u64, reply: Reply, result: anyhow::Result<Value>) {
        if generation != self.generation {
            return;
        }
        if result.as_ref().is_err_and(|err| err.is::<auth::ReLogin>()) {
            self.forget_account();
            self.set_status("Bitte neu anmelden – „Anmelden“ klicken");
        }
        match (reply, result) {
            (Reply::Done(context), Ok(_)) => self.host.ok(&context),
            (Reply::Clip(context, open), Ok(body)) => {
                self.host.ok(&context);
                if let Some(url) = body["data"][0]["edit_url"].as_str().filter(|_| open) {
                    self.open_url(url);
                }
            }
            (Reply::Chat(context, mode, on), Ok(_)) => {
                self.chat[mode.as_str()] = json!(on);
                self.host.ok(&context);
                self.refresh_all();
            }
            (Reply::ChatRead, Ok(body)) => {
                self.chat = body["data"][0].clone();
                self.refresh_all();
            }
            (Reply::Viewers, Ok(body)) => {
                self.stream = stream_state(&body);
                self.refresh_all();
            }
            (reply, Err(err)) => {
                if let Some(context) = reply.context() {
                    self.host.alert(context);
                }
                self.host.log(format!("{err:#}"));
            }
        }
    }

    fn read_chat(&self) {
        if let Some(session) = self.session() {
            let path = chat_path(&session.user_id);
            self.spawn_api(
                session,
                Reply::ChatRead,
                Req {
                    method: reqwest::Method::GET,
                    path,
                    body: None,
                },
            );
        }
    }

    /// Fetches the viewer count if the key is shown and the last poll is old enough.
    fn poll_viewers(&mut self) {
        let due = self.last_poll.is_none_or(|at| at.elapsed() >= VIEWER_POLL);
        let Some(session) = self
            .session()
            .filter(|_| due && self.contexts.any("viewers"))
        else {
            return;
        };
        self.last_poll = Some(Instant::now());
        let path = format!("/streams?user_id={}", session.user_id);
        self.spawn_api(
            session,
            Reply::Viewers,
            Req {
                method: reqwest::Method::GET,
                path,
                body: None,
            },
        );
    }

    fn spawn_api(&self, session: Session, reply: Reply, req: Req) {
        let (http, tx, generation) = (self.http.clone(), self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let result = helix(&http, &tx, generation, session, &req).await;
            tx.send(Msg::Api(generation, reply, result)).ok();
        });
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            if msg["event"] == "willAppear" {
                if self
                    .contexts
                    .get(&context)
                    .is_some_and(|inst| inst.kind() == "chatmode")
                {
                    self.read_chat();
                }
                self.poll_viewers();
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
        if inst.kind() == "viewers" {
            let login = global_str(&self.global, "login");
            if login.is_empty() {
                self.host.alert(context);
                self.host
                    .log("Erst anmelden, dann öffnet die Taste den Kanal");
            } else {
                self.open_url(&format!("https://www.twitch.tv/{login}"));
            }
            return;
        }
        let Some(session) = self.session() else {
            self.host.alert(context);
            self.host
                .log("Nicht angemeldet – Plugins → Twitch → Einstellungen → Anmelden");
            return;
        };
        let user_id = session.user_id.clone();
        match inst.kind() {
            "marker" => {
                let mut body = json!({ "user_id": user_id });
                let description = inst.text("description");
                if !description.is_empty() {
                    body["description"] = json!(description);
                }
                self.spawn_api(
                    session,
                    Reply::Done(context.into()),
                    Req {
                        method: reqwest::Method::POST,
                        path: "/streams/markers".into(),
                        body: Some(body),
                    },
                );
            }
            "clip" => self.spawn_api(
                session,
                Reply::Clip(context.into(), inst.flag("open", false)),
                Req {
                    method: reqwest::Method::POST,
                    path: format!("/clips?broadcaster_id={user_id}"),
                    body: None,
                },
            ),
            "chatmode" => {
                let mode = inst.text_or("mode", "slow_mode");
                let on = !chat_on(&self.chat, &mode);
                let body = chat_body(&mode, on);
                self.spawn_api(
                    session,
                    Reply::Chat(context.into(), mode, on),
                    Req {
                        method: reqwest::Method::PATCH,
                        path: chat_path(&user_id),
                        body: Some(body),
                    },
                );
            }
            "ad" => {
                let body = json!({
                    "broadcaster_id": user_id,
                    "length": inst.number("length", 30.0) as u32,
                });
                self.spawn_api(
                    session,
                    Reply::Done(context.into()),
                    Req {
                        method: reqwest::Method::POST,
                        path: "/channels/commercial".into(),
                        body: Some(body),
                    },
                );
            }
            _ => {}
        }
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        match inst.kind() {
            "viewers" => (
                0,
                match self.stream {
                    Stream::Unknown => String::new(),
                    Stream::Offline => "offline".into(),
                    Stream::Live(count) => viewer_title(count),
                },
            ),
            "chatmode" => {
                let mode = inst.text_or("mode", "slow_mode");
                (
                    u16::from(chat_on(&self.chat, &mode)),
                    chat_label(&mode).into(),
                )
            }
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

    fn on_tick(&mut self) {
        self.poll_viewers();
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
            Msg::Api(generation, reply, result) => self.on_api(generation, reply, result),
        }
    }
}

async fn sign_in(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    client_id: &str,
) -> anyhow::Result<Account> {
    let device = auth::device_code(http, client_id).await?;
    tx.send(Msg::Code(generation, device.clone())).ok();
    let tokens = auth::poll(http, client_id, &device).await?;
    let (user_id, login) = auth::account(http, client_id, &tokens.access_token).await?;
    Ok(Account {
        user_id,
        login,
        tokens,
    })
}

/// Runs one Helix request. The token is refreshed before it expires and once
/// more after a 401; new tokens are handed back to the plugin.
async fn helix(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    mut session: Session,
    req: &Req,
) -> anyhow::Result<Value> {
    if auth::needs_refresh(session.tokens.expires_at, auth::now()) {
        refresh(http, tx, generation, &mut session).await?;
    }
    let (mut code, mut body) = send(http, &session, req).await?;
    if code == 401 {
        refresh(http, tx, generation, &mut session).await?;
        (code, body) = send(http, &session, req).await?;
    }
    anyhow::ensure!((200..300).contains(&code), "{}", twitch_error(code, &body));
    Ok(body)
}

async fn refresh(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    session: &mut Session,
) -> anyhow::Result<()> {
    // Twitch refresh tokens work once. Requests started with the same old token
    // (e.g. several keys after the PC wakes up) must share one refresh.
    static LAST: tokio::sync::Mutex<Option<(String, auth::Tokens)>> =
        tokio::sync::Mutex::const_new(None);
    let mut last = LAST.lock().await;
    if let Some((old, tokens)) = last.as_ref()
        && *old == session.tokens.refresh_token
    {
        session.tokens = tokens.clone();
        return Ok(());
    }
    let tokens = auth::refresh(http, &session.client_id, &session.tokens.refresh_token).await?;
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
        .request(req.method.clone(), format!("{HELIX}{}", req.path))
        .bearer_auth(&session.tokens.access_token)
        .header("Client-Id", &session.client_id);
    if let Some(body) = &req.body {
        call = call.json(body);
    }
    let res = call.send().await.context("Twitch nicht erreichbar")?;
    let code = res.status().as_u16();
    let body = serde_json::from_slice(&res.bytes().await?).unwrap_or_default();
    Ok((code, body))
}

fn twitch_error(code: u16, body: &Value) -> String {
    match body["message"].as_str().filter(|m| !m.is_empty()) {
        Some(message) => format!("Twitch antwortet mit HTTP {code}: {message}"),
        None => format!("Twitch antwortet mit HTTP {code}"),
    }
}

fn chat_path(user_id: &str) -> String {
    format!("/chat/settings?broadcaster_id={user_id}&moderator_id={user_id}")
}

/// Body that sets one chat mode, e.g. `{"slow_mode": true}`.
fn chat_body(mode: &str, on: bool) -> Value {
    json!({ mode: on })
}

fn chat_on(settings: &Value, mode: &str) -> bool {
    settings[mode].as_bool().unwrap_or(false)
}

fn chat_label(mode: &str) -> &str {
    match mode {
        "slow_mode" => "Langsam",
        "follower_mode" => "Follower",
        "subscriber_mode" => "Abo",
        "emote_mode" => "Emotes",
        "unique_chat_mode" => "Unique",
        other => other,
    }
}

fn stream_state(body: &Value) -> Stream {
    body["data"][0]["viewer_count"]
        .as_u64()
        .map_or(Stream::Offline, Stream::Live)
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
        .user_agent(concat!("OpenDeckN3-Twitch/", env!("CARGO_PKG_VERSION")))
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
            _ = tick.tick() => plugin.on_tick(),
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
    fn stream_lookup_offline_or_live() {
        assert!(matches!(
            stream_state(&json!({ "data": [] })),
            Stream::Offline
        ));
        assert!(matches!(
            stream_state(&json!({ "data": [{ "viewer_count": 42 }] })),
            Stream::Live(42)
        ));
    }

    #[test]
    fn chat_mode_body_and_state() {
        assert_eq!(chat_body("emote_mode", true), json!({ "emote_mode": true }));
        let settings = json!({ "slow_mode": true, "emote_mode": false });
        assert!(chat_on(&settings, "slow_mode"));
        assert!(!chat_on(&settings, "emote_mode"));
        assert!(!chat_on(&Value::Null, "slow_mode"));
        assert_eq!(
            chat_path("42"),
            "/chat/settings?broadcaster_id=42&moderator_id=42"
        );
    }
}

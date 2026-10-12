#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 Spotify plugin: like the playing track, add it to a playlist,
//! toggle shuffle and repeat, and move playback to another device over the
//! Spotify Web API (see `docs/PLUGIN_SPOTIFY.md`). Play, pause and cover stay
//! in the media plugin.

mod auth;

use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const API: &str = "https://api.spotify.com/v1";
/// Playback is polled this often, only while a key is shown.
const PLAYER_POLL: Duration = Duration::from_secs(5);
/// Global settings written by a sign-in; cleared on sign-out.
const ACCOUNT_KEYS: [&str; 5] = [
    "accessToken",
    "refreshToken",
    "expiresAt",
    "userId",
    "account",
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
    name: String,
    tokens: auth::Tokens,
}

/// One Web API request: method, path below `/v1` (with query) and optional JSON body.
struct Req {
    method: reqwest::Method,
    path: String,
    body: Option<Value>,
}

/// Spotify answered 429 (`Retry-After`) or the app's quota is used up.
#[derive(Debug)]
enum Limit {
    Retry(u64),
    Quota,
}

impl std::fmt::Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Limit::Retry(secs) => write!(f, "Spotify bremst – in {secs} s erneut versuchen"),
            Limit::Quota => f.write_str("Spotify-Kontingent erschöpft – später erneut versuchen"),
        }
    }
}

impl std::error::Error for Limit {}

/// What a finished API request means for the plugin.
enum Reply {
    /// Player state from the poll.
    Player,
    /// Saved state of the track with this URI.
    Saved(String),
    /// Like toggled for the track with this URI; the new state is the flag.
    Like(String, String, bool),
    /// Action done: flash the key.
    Done(String),
    /// Playlist or device list for a settings dropdown; holds the request and the user id.
    Options(Value, String),
}

impl Reply {
    fn context(&self) -> Option<&str> {
        match self {
            Reply::Like(context, _, _) | Reply::Done(context) => Some(context),
            _ => None,
        }
    }
}

enum Msg {
    Url(u64, String),
    Signed(u64, anyhow::Result<Account>),
    Tokens(u64, auth::Tokens),
    Api(u64, Reply, anyhow::Result<Value>),
}

/// Playback as of the last poll. `None` in the plugin means nothing is playing.
#[derive(Default)]
struct Playback {
    /// `spotify:track:<id>` of the current item; `None` for episodes and local files.
    track: Option<String>,
    /// Whether `track` is in the library; `None` until read.
    saved: Option<bool>,
    shuffle: bool,
    /// `off`, `context` or `track`.
    repeat: String,
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
    playback: Option<Playback>,
    last_poll: Option<Instant>,
    /// No polling until this instant after a 429.
    backoff: Option<Instant>,
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
            playback: None,
            last_poll: None,
            backoff: None,
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

    fn fail(&self, context: &str, why: &str) {
        self.host.alert(context);
        self.host.log(why);
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
            format!("Verbunden als {}", global_str(&self.global, "account"))
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
        self.playback = None;
        self.last_poll = None;
        self.backoff = None;
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

    fn on_url(&mut self, generation: u64, url: String) {
        if generation == self.generation {
            self.open_url(&url);
        }
    }

    fn on_signed(&mut self, generation: u64, result: anyhow::Result<Account>) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(account) => {
                self.store_tokens(&account.tokens);
                self.global.insert("userId".into(), json!(account.user_id));
                self.global.insert("account".into(), json!(account.name));
                self.set_status(format!("Verbunden als {}", account.name));
                self.last_poll = None;
                self.poll_player();
            }
            Err(err) => self.set_status(format!("Anmeldung fehlgeschlagen: {err:#}")),
        }
    }

    fn on_api(&mut self, generation: u64, reply: Reply, result: anyhow::Result<Value>) {
        if generation != self.generation {
            return;
        }
        match (reply, result) {
            (Reply::Options(req, user_id), result) => {
                let options = result.map(|body| match req["request"].as_str() {
                    Some("devices") => device_options(&body),
                    _ => playlist_options(&body, &user_id),
                });
                self.host.reply_options(&req, options);
            }
            (Reply::Player, Ok(body)) => self.on_player(body),
            (Reply::Saved(uri), Ok(body)) => self.set_saved(&uri, body[0].as_bool()),
            (Reply::Like(context, uri, saved), Ok(_)) => {
                self.set_saved(&uri, Some(saved));
                self.host.ok(&context);
            }
            (Reply::Done(context), Ok(_)) => {
                self.host.ok(&context);
                self.last_poll = None;
            }
            (reply, Err(err)) => {
                if err.is::<auth::ReLogin>() {
                    self.forget_account();
                    self.set_status("Bitte neu anmelden – „Anmelden“ klicken");
                } else if let Some(limit) = err.downcast_ref::<Limit>() {
                    match limit {
                        Limit::Retry(secs) => {
                            self.backoff = Some(Instant::now() + Duration::from_secs(*secs));
                        }
                        Limit::Quota => self.set_status(limit.to_string()),
                    }
                }
                if let Some(context) = reply.context() {
                    self.host.alert(context);
                }
                self.host.log(format!("{err:#}"));
            }
        }
    }

    fn on_player(&mut self, body: Value) {
        if body.is_null() {
            self.playback = None;
            self.refresh_all();
            return;
        }
        let track = track_uri_of(&body["item"]);
        let previous = self.playback.take().unwrap_or_default();
        let same = previous.track == track;
        if let Some(uri) = track.as_ref().filter(|_| !same) {
            self.read_saved(uri.clone());
        }
        self.playback = Some(Playback {
            track,
            saved: if same { previous.saved } else { None },
            shuffle: body["shuffle_state"].as_bool().unwrap_or(false),
            repeat: body["repeat_state"].as_str().unwrap_or("off").to_owned(),
        });
        self.refresh_all();
    }

    /// Stores the saved flag only if the track is still the current one.
    fn set_saved(&mut self, uri: &str, saved: Option<bool>) {
        if let Some(playback) = &mut self.playback
            && playback.track.as_deref() == Some(uri)
        {
            playback.saved = saved;
        }
        self.refresh_all();
    }

    fn read_saved(&self, uri: String) {
        if let Some(session) = self.session() {
            let path = format!("/me/library/contains?uris={uri}");
            self.spawn_api(
                session,
                Reply::Saved(uri),
                Req {
                    method: reqwest::Method::GET,
                    path,
                    body: None,
                },
            );
        }
    }

    /// Fetches the player state if a key is shown and the last poll is old enough.
    fn poll_player(&mut self) {
        let due = self.last_poll.is_none_or(|at| at.elapsed() >= PLAYER_POLL);
        let backed_off = self.backoff.is_some_and(|until| Instant::now() < until);
        if !due || backed_off || self.contexts.iter().next().is_none() {
            return;
        }
        let Some(session) = self.session() else {
            return;
        };
        self.last_poll = Some(Instant::now());
        self.spawn_api(
            session,
            Reply::Player,
            Req {
                method: reqwest::Method::GET,
                path: "/me/player".into(),
                body: None,
            },
        );
    }

    fn spawn_api(&self, session: Session, reply: Reply, req: Req) {
        let (http, tx, generation) = (self.http.clone(), self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let result = api(&http, &tx, generation, session, &req).await;
            tx.send(Msg::Api(generation, reply, result)).ok();
        });
    }

    /// Playlist or device list for a settings dropdown.
    fn on_request(&self, req: Value) {
        let path = match req["request"].as_str() {
            Some("playlists") => "/me/playlists?limit=50",
            Some("devices") => "/me/player/devices",
            other => {
                self.host
                    .reply_options(&req, Err(anyhow!("unbekannte Anfrage {other:?}")));
                return;
            }
        };
        let Some(session) = self.session() else {
            self.host.reply_options(
                &req,
                Err(anyhow!(
                    "Nicht angemeldet – zuerst in den Einstellungen anmelden"
                )),
            );
            return;
        };
        let user_id = session.user_id.clone();
        self.spawn_api(
            session,
            Reply::Options(req, user_id),
            Req {
                method: reqwest::Method::GET,
                path: path.into(),
                body: None,
            },
        );
    }

    fn on_host(&mut self, msg: Value) {
        if self.contexts.apply(&msg).is_some() {
            if msg["event"] == "willAppear" {
                self.poll_player();
            }
            self.refresh_all();
            return;
        }
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "keyDown" => self.press(msg["context"].as_str().unwrap_or_default()),
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            _ => {}
        }
    }

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let Some(session) = self.session() else {
            self.fail(
                context,
                "Nicht angemeldet – Plugins → Spotify → Einstellungen → Anmelden",
            );
            return;
        };
        let playback = self.playback.as_ref();
        match inst.kind() {
            "like" => {
                let Some((uri, saved)) = playback.and_then(|p| p.track.clone().zip(p.saved)) else {
                    self.fail(
                        context,
                        "Kein Titel bekannt – zuerst etwas in Spotify abspielen",
                    );
                    return;
                };
                let (method, next) = if saved {
                    (reqwest::Method::DELETE, false)
                } else {
                    (reqwest::Method::PUT, true)
                };
                self.spawn_api(
                    session,
                    Reply::Like(context.into(), uri.clone(), next),
                    Req {
                        method,
                        path: format!("/me/library?uris={uri}"),
                        body: None,
                    },
                );
            }
            "addtoplaylist" => {
                let Some(uri) = playback.and_then(|p| p.track.clone()) else {
                    self.fail(
                        context,
                        "Kein Titel bekannt – zuerst etwas in Spotify abspielen",
                    );
                    return;
                };
                let playlist = inst.text("playlist");
                if playlist.is_empty() {
                    self.fail(
                        context,
                        "Keine Playlist gewählt – in den Tasteneinstellungen auswählen",
                    );
                    return;
                }
                self.spawn_api(
                    session,
                    Reply::Done(context.into()),
                    Req {
                        method: reqwest::Method::POST,
                        path: format!("/playlists/{playlist}/items"),
                        body: Some(json!({ "uris": [uri] })),
                    },
                );
            }
            "shuffle" => {
                let Some(playback) = playback else {
                    self.fail(context, "Keine Wiedergabe in Spotify aktiv");
                    return;
                };
                self.spawn_api(
                    session,
                    Reply::Done(context.into()),
                    Req {
                        method: reqwest::Method::PUT,
                        path: format!("/me/player/shuffle?state={}", !playback.shuffle),
                        body: None,
                    },
                );
            }
            "repeat" => {
                let Some(playback) = playback else {
                    self.fail(context, "Keine Wiedergabe in Spotify aktiv");
                    return;
                };
                self.spawn_api(
                    session,
                    Reply::Done(context.into()),
                    Req {
                        method: reqwest::Method::PUT,
                        path: format!("/me/player/repeat?state={}", next_repeat(&playback.repeat)),
                        body: None,
                    },
                );
            }
            "device" => {
                let device = inst.text("device");
                if device.is_empty() {
                    self.fail(
                        context,
                        "Kein Gerät gewählt – in den Tasteneinstellungen auswählen",
                    );
                    return;
                }
                self.spawn_api(
                    session,
                    Reply::Done(context.into()),
                    Req {
                        method: reqwest::Method::PUT,
                        path: "/me/player".into(),
                        body: Some(json!({ "device_ids": [device] })),
                    },
                );
            }
            _ => {}
        }
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        let playback = self.playback.as_ref();
        let (state, dash) = match (inst.kind(), playback) {
            ("like", Some(p)) => (u16::from(p.saved == Some(true)), false),
            ("shuffle", Some(p)) => (u16::from(p.shuffle), false),
            ("repeat", Some(p)) => (repeat_state(&p.repeat), false),
            ("like" | "shuffle" | "repeat", None) => (0, true),
            _ => (0, false),
        };
        (state, if dash { "–" } else { "" }.to_owned())
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
        self.poll_player();
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Url(generation, url) => self.on_url(generation, url),
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
    let tokens = auth::sign_in(http, client_id, |url| {
        tx.send(Msg::Url(generation, url)).ok();
    })
    .await?;
    let (user_id, name) = auth::profile(http, &tokens.access_token).await?;
    Ok(Account {
        user_id,
        name,
        tokens,
    })
}

/// Runs one Web API request. The token is refreshed before it expires and
/// once more after a 401; new tokens are handed back to the plugin.
async fn api(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    mut session: Session,
    req: &Req,
) -> anyhow::Result<Value> {
    if auth::needs_refresh(session.tokens.expires_at, auth::now()) {
        refresh(http, tx, generation, &mut session).await?;
    }
    let (mut code, mut retry, mut body) = send(http, &session, req).await?;
    if code == 401 {
        refresh(http, tx, generation, &mut session).await?;
        (code, retry, body) = send(http, &session, req).await?;
    }
    if (200..300).contains(&code) {
        return Ok(body);
    }
    Err(spotify_error(code, retry, &body))
}

async fn refresh(
    http: &reqwest::Client,
    tx: &mpsc::UnboundedSender<Msg>,
    generation: u64,
    session: &mut Session,
) -> anyhow::Result<()> {
    // Spotify refresh requests started with the same old token (e.g. several
    // keys after the PC wakes up) must share one refresh.
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

/// Status, `Retry-After` in seconds (default 1) and the JSON body.
async fn send(
    http: &reqwest::Client,
    session: &Session,
    req: &Req,
) -> anyhow::Result<(u16, u64, Value)> {
    let mut call = http
        .request(req.method.clone(), format!("{API}{}", req.path))
        .bearer_auth(&session.tokens.access_token);
    if let Some(body) = &req.body {
        call = call.json(body);
    }
    let res = call.send().await.context("Spotify nicht erreichbar")?;
    let code = res.status().as_u16();
    let retry = res
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1);
    let body: Value = serde_json::from_slice(&res.bytes().await?).unwrap_or_default();
    Ok((code, retry, body))
}

fn spotify_error(code: u16, retry: u64, body: &Value) -> anyhow::Error {
    match (code, body["error"]["reason"].as_str()) {
        (429, Some("QUOTA_EXCEEDED")) => Limit::Quota.into(),
        (429, _) => Limit::Retry(retry).into(),
        _ => anyhow!(
            "Spotify antwortet mit HTTP {code}: {}",
            body["error"]["message"].as_str().unwrap_or("ohne Meldung")
        ),
    }
}

/// `spotify:track:<id>` of a track item; `None` for episodes and local files.
fn track_uri_of(item: &Value) -> Option<String> {
    let id = item["id"].as_str().filter(|_| item["type"] == "track")?;
    Some(format!("spotify:track:{id}"))
}

/// Order of the repeat button: off → context → track → off.
fn next_repeat(state: &str) -> &'static str {
    match state {
        "off" => "context",
        "context" => "track",
        _ => "off",
    }
}

/// Icon state of the repeat key: 0 off, 1 whole context, 2 current track.
fn repeat_state(state: &str) -> u16 {
    match state {
        "context" => 1,
        "track" => 2,
        _ => 0,
    }
}

/// Playlists the account may add tracks to: own or collaborative ones.
fn playlist_options(body: &Value, user_id: &str) -> Vec<(String, String)> {
    body["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["owner"]["id"] == user_id || p["collaborative"].as_bool() == Some(true))
        .filter_map(|p| Some((p["id"].as_str()?.to_owned(), p["name"].as_str()?.to_owned())))
        .collect()
}

fn device_options(body: &Value) -> Vec<(String, String)> {
    body["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| Some((d["id"].as_str()?.to_owned(), d["name"].as_str()?.to_owned())))
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let http = reqwest::Client::builder()
        .user_agent(concat!("OpenDeckN3-Spotify/", env!("CARGO_PKG_VERSION")))
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
    fn repeat_cycles_off_context_track() {
        assert_eq!(next_repeat("off"), "context");
        assert_eq!(next_repeat("context"), "track");
        assert_eq!(next_repeat("track"), "off");
        assert_eq!(next_repeat(""), "off");
        assert_eq!(repeat_state("off"), 0);
        assert_eq!(repeat_state("context"), 1);
        assert_eq!(repeat_state("track"), 2);
    }

    #[test]
    fn track_uri_only_for_tracks() {
        let track = json!({ "type": "track", "id": "4uLU6hMCjMI75M1A2tKUQC" });
        assert_eq!(
            track_uri_of(&track).as_deref(),
            Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC")
        );
        assert_eq!(
            track_uri_of(&json!({ "type": "episode", "id": "e1" })),
            None
        );
        assert_eq!(track_uri_of(&Value::Null), None);
    }

    #[test]
    fn playlists_you_can_add_to() {
        let body = json!({ "items": [
            { "id": "p1", "name": "Mine", "owner": { "id": "me" }, "collaborative": false },
            { "id": "p2", "name": "Theirs", "owner": { "id": "other" }, "collaborative": false },
            { "id": "p3", "name": "Shared", "owner": { "id": "other" }, "collaborative": true },
            null
        ]});
        assert_eq!(
            playlist_options(&body, "me"),
            [("p1", "Mine"), ("p3", "Shared")]
                .map(|(id, name)| (id.to_owned(), name.to_owned()))
                .to_vec()
        );
    }
}

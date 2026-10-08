//! OpenDeckN3 Discord plugin.
//!
//! Talks to OpenDeckN3 over the Stream Deck SDK WebSocket protocol and to the
//! local Discord app over its RPC/IPC interface (see `docs/PLUGIN_DISCORD.md`).

mod auth;
mod ipc;

use std::{collections::HashMap, time::Duration};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use auth::{Credentials, NeedsAuthorize, Session, Tokens};
use ipc::{IpcEvent, Rpc};

const PREFIX: &str = "de.opendeckn3.discord.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Mute,
    Deafen,
    VoiceChannel,
    TextChannel,
    Notifications,
    UserVolume,
    Volume,
    Soundboard,
    AudioDevice,
    Camera,
    Screenshare,
    PushToTalk,
    VoiceMode,
}

impl Act {
    fn from_uuid(uuid: &str) -> Option<Self> {
        Some(match uuid.strip_prefix(PREFIX)? {
            "mute" => Self::Mute,
            "deafen" => Self::Deafen,
            "voicechannel" => Self::VoiceChannel,
            "textchannel" => Self::TextChannel,
            "notifications" => Self::Notifications,
            "uservolume" => Self::UserVolume,
            "volume" => Self::Volume,
            "soundboard" => Self::Soundboard,
            "audiodevice" => Self::AudioDevice,
            "camera" => Self::Camera,
            "screenshare" => Self::Screenshare,
            "pushtotalk" => Self::PushToTalk,
            "voicemode" => Self::VoiceMode,
            _ => return None,
        })
    }
}

struct Instance {
    act: Act,
    settings: Value,
    /// Last state/title sent, to avoid redundant updates.
    shown: Option<(u16, String)>,
}

impl Instance {
    fn text(&self, key: &str) -> String {
        match &self.settings[key] {
            Value::String(s) => s.trim().to_owned(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        }
    }

    fn number(&self, key: &str, default: f64) -> f64 {
        match &self.settings[key] {
            Value::Number(n) => n.as_f64().unwrap_or(default),
            Value::String(s) => s.trim().parse().unwrap_or(default),
            _ => default,
        }
    }

    fn mode(&self, default: &str) -> String {
        Some(self.text("mode"))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| default.to_owned())
    }
}

enum Msg {
    Host(Value),
    HostClosed,
    Ipc(u64, IpcEvent),
    Connected(u64, anyhow::Result<Session>),
    /// Result of GET_SELECTED_VOICE_CHANNEL (voice states for user volume).
    Channel(u64, Value),
    VoiceSettings(u64, Value),
    CallFailed(Option<String>, String),
}

struct Plugin {
    uuid: String,
    host: mpsc::UnboundedSender<Value>,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: HashMap<String, Instance>,
    global: Map<String, Value>,
    loaded: bool,
    creds: Credentials,
    rpc: Option<Rpc>,
    /// Increments per connection attempt; stale messages are dropped.
    generation: u64,
    connecting: bool,
    /// Saved tokens were rejected: wait for the user instead of retrying.
    needs_authorize: bool,
    voice: Value,
    channel: Option<String>,
    user_volumes: HashMap<String, (f64, bool)>,
    notifications: u32,
    video: bool,
    screenshare: bool,
    ptt_restore: Option<bool>,
}

impl Plugin {
    fn new(
        uuid: String,
        host: mpsc::UnboundedSender<Value>,
        tx: mpsc::UnboundedSender<Msg>,
    ) -> Self {
        Self {
            uuid,
            host,
            tx,
            contexts: HashMap::new(),
            global: Map::new(),
            loaded: false,
            creds: Credentials::default(),
            rpc: None,
            generation: 0,
            connecting: false,
            needs_authorize: false,
            voice: Value::Null,
            channel: None,
            user_volumes: HashMap::new(),
            notifications: 0,
            video: false,
            screenshare: false,
            ptt_restore: None,
        }
    }

    fn send(&self, event: Value) {
        self.host.send(event).ok();
    }

    fn log(&self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("discord: {message}");
        self.send(json!({ "event": "logMessage", "payload": { "message": message } }));
    }

    fn alert(&self, context: &str) {
        self.send(json!({ "event": "showAlert", "context": context }));
    }

    // ------------------------------------------------------------------
    // Global settings, connection
    // ------------------------------------------------------------------

    fn global_str(&self, key: &str) -> String {
        self.global
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    fn tokens(&self) -> Option<Tokens> {
        let access_token = self.global_str("accessToken");
        (!access_token.is_empty()).then(|| Tokens {
            access_token,
            refresh_token: self.global_str("refreshToken"),
            expires_at: self
                .global
                .get("expiresAt")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        })
    }

    fn save_global(&self) {
        self.send(json!({
            "event": "setGlobalSettings",
            "context": self.uuid,
            "payload": Value::Object(self.global.clone()),
        }));
    }

    fn set_status(&mut self, status: impl Into<String>) {
        let status = status.into();
        if self.global.get("status").and_then(Value::as_str) != Some(status.as_str()) {
            self.global.insert("status".into(), json!(status));
            self.save_global();
        }
    }

    fn on_global_settings(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        let redirect = self.global_str("redirectUri");
        let creds = Credentials {
            client_id: self.global_str("clientId"),
            client_secret: self.global_str("clientSecret"),
            redirect_uri: if redirect.is_empty() {
                "http://localhost".into()
            } else {
                redirect
            },
        };
        let changed = |key: &str| previous.get(key) != self.global.get(key);
        let first = !self.loaded;
        self.loaded = true;

        if first {
            self.creds = creds;
            if !self.creds.complete() {
                self.set_status("Client-ID und Client-Secret eintragen");
            } else if self.tokens().is_some() {
                self.start_connect(false);
            } else {
                self.set_status("Nicht verbunden – „Mit Discord verbinden“ klicken");
            }
            return;
        }
        if changed("signOut") {
            self.disconnect();
            for key in ["accessToken", "refreshToken", "expiresAt", "user"] {
                self.global.remove(key);
            }
            self.set_status("Abgemeldet");
            self.save_global();
            return;
        }
        if creds != self.creds {
            let other_app = creds.client_id != self.creds.client_id;
            self.creds = creds;
            self.disconnect();
            if other_app {
                for key in ["accessToken", "refreshToken", "expiresAt", "user"] {
                    self.global.remove(key);
                }
            }
            if self.creds.complete() {
                self.start_connect(true);
            } else {
                self.set_status("Client-ID und Client-Secret eintragen");
            }
            return;
        }
        if changed("connect") {
            self.disconnect();
            if self.creds.complete() {
                self.start_connect(true);
            } else {
                self.set_status("Client-ID und Client-Secret eintragen");
            }
        }
    }

    fn disconnect(&mut self) {
        if let Some(rpc) = self.rpc.take() {
            rpc.close();
        }
        self.generation += 1;
        self.connecting = false;
        self.voice = Value::Null;
        self.channel = None;
        self.refresh_all();
    }

    fn start_connect(&mut self, allow_authorize: bool) {
        if self.connecting || !self.creds.complete() {
            return;
        }
        self.generation += 1;
        self.connecting = true;
        let generation = self.generation;
        self.set_status(if allow_authorize {
            "Verbinde … bitte in Discord bestätigen"
        } else {
            "Verbinde …"
        });
        let (creds, tokens, tx) = (self.creds.clone(), self.tokens(), self.tx.clone());
        tokio::spawn(async move {
            let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
            let forward = tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = ev_rx.recv().await {
                    if forward.send(Msg::Ipc(generation, ev)).is_err() {
                        break;
                    }
                }
            });
            let result = auth::connect(&creds, tokens, allow_authorize, ev_tx).await;
            tx.send(Msg::Connected(generation, result)).ok();
        });
    }

    fn on_connected(&mut self, generation: u64, result: anyhow::Result<Session>) {
        if generation != self.generation {
            if let Ok(session) = result {
                session.rpc.close();
            }
            return;
        }
        self.connecting = false;
        let session = match result {
            Ok(session) => session,
            Err(err) => {
                if err.is::<NeedsAuthorize>() {
                    self.needs_authorize = true;
                }
                self.set_status(format!("Nicht verbunden: {err:#}"));
                return;
            }
        };
        self.needs_authorize = false;
        self.global
            .insert("accessToken".into(), json!(session.tokens.access_token));
        self.global
            .insert("refreshToken".into(), json!(session.tokens.refresh_token));
        self.global
            .insert("expiresAt".into(), json!(session.tokens.expires_at));
        self.global.insert("user".into(), json!(session.user));
        self.global.insert(
            "status".into(),
            json!(format!("Verbunden als {}", session.user)),
        );
        self.save_global();
        self.log(format!("connected as {}", session.user));

        let rpc = session.rpc;
        self.rpc = Some(rpc.clone());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            for evt in [
                "VOICE_SETTINGS_UPDATE",
                "VOICE_CHANNEL_SELECT",
                "NOTIFICATION_CREATE",
                "VIDEO_STATE_UPDATE",
                "SCREENSHARE_STATE_UPDATE",
            ] {
                if let Err(err) = rpc.subscribe(evt, json!({})).await {
                    eprintln!("discord: subscribe {evt}: {err:#}");
                }
            }
            if let Ok(voice) = rpc.call("GET_VOICE_SETTINGS", json!({})).await {
                tx.send(Msg::VoiceSettings(generation, voice)).ok();
            }
            if let Ok(channel) = rpc.call("GET_SELECTED_VOICE_CHANNEL", json!({})).await {
                tx.send(Msg::Channel(generation, channel)).ok();
            }
        });
    }

    fn on_ipc(&mut self, generation: u64, event: IpcEvent) {
        if generation != self.generation {
            return;
        }
        match event {
            IpcEvent::Closed(reason) => {
                if self.rpc.take().is_some() {
                    self.log(format!("Discord connection closed: {reason}"));
                    self.set_status("Discord nicht erreichbar – verbinde erneut …");
                }
                self.voice = Value::Null;
                self.channel = None;
                self.refresh_all();
            }
            IpcEvent::Dispatch(evt, data) => match evt.as_str() {
                "VOICE_SETTINGS_UPDATE" => {
                    self.voice = data;
                    self.refresh_all();
                }
                "VOICE_CHANNEL_SELECT" => {
                    let channel = data["channel_id"].as_str().map(str::to_owned);
                    self.channel = channel.clone();
                    self.user_volumes.clear();
                    self.refresh_all();
                    if channel.is_some() {
                        self.fetch_channel();
                    }
                }
                "NOTIFICATION_CREATE" => {
                    self.notifications += 1;
                    self.refresh_all();
                }
                "VIDEO_STATE_UPDATE" => {
                    self.video = data["active"].as_bool().unwrap_or(false);
                    self.refresh_all();
                }
                "SCREENSHARE_STATE_UPDATE" => {
                    self.screenshare = data["active"].as_bool().unwrap_or(false);
                    self.refresh_all();
                }
                _ => {}
            },
        }
    }

    fn on_channel(&mut self, generation: u64, channel: Value) {
        if generation != self.generation {
            return;
        }
        self.channel = channel["id"].as_str().map(str::to_owned);
        for state in channel["voice_states"].as_array().into_iter().flatten() {
            if let Some(id) = state["user"]["id"].as_str() {
                let volume = state["volume"].as_f64().unwrap_or(100.0);
                let mute = state["mute"].as_bool().unwrap_or(false);
                self.user_volumes.insert(id.to_owned(), (volume, mute));
            }
        }
        self.refresh_all();
    }

    fn fetch_channel(&self) {
        let (Some(rpc), tx, generation) = (self.rpc.clone(), self.tx.clone(), self.generation)
        else {
            return;
        };
        tokio::spawn(async move {
            if let Ok(channel) = rpc.call("GET_SELECTED_VOICE_CHANNEL", json!({})).await {
                tx.send(Msg::Channel(generation, channel)).ok();
            }
        });
    }

    /// Reconnects silently while saved tokens exist.
    fn on_tick(&mut self) {
        if self.rpc.is_none()
            && !self.connecting
            && !self.needs_authorize
            && self.creds.complete()
            && self.tokens().is_some()
        {
            self.start_connect(false);
        }
    }

    // ------------------------------------------------------------------
    // Key state
    // ------------------------------------------------------------------

    fn voice_bool(&self, key: &str) -> bool {
        self.voice[key].as_bool().unwrap_or(false)
    }

    fn volume_of(&self, target: &str) -> f64 {
        self.voice[target]["volume"].as_f64().unwrap_or(100.0)
    }

    /// State index (0 = normal, 1 = active/muted) and title for a key.
    fn appearance(&self, inst: &Instance) -> (u16, String) {
        let on = |b: bool| u16::from(b);
        let connected = self.rpc.is_some() && !self.voice.is_null();
        if !connected {
            return (0, String::new());
        }
        match inst.act {
            Act::Mute => (
                on(self.voice_bool("mute") || self.voice_bool("deaf")),
                String::new(),
            ),
            Act::Deafen => (on(self.voice_bool("deaf")), String::new()),
            Act::VoiceChannel => {
                let target = inst.text("channel");
                (
                    on(!target.is_empty() && self.channel.as_deref() == Some(&target)),
                    String::new(),
                )
            }
            Act::Notifications => (
                on(self.notifications > 0),
                if self.notifications > 0 {
                    self.notifications.to_string()
                } else {
                    String::new()
                },
            ),
            Act::Volume => {
                let target = target_of(inst);
                let muted = if target == "input" {
                    self.voice_bool("mute")
                } else {
                    self.voice_bool("deaf")
                };
                (on(muted), format!("{} %", self.volume_of(target).round()))
            }
            Act::UserVolume => match self.user_volumes.get(&inst.text("user")) {
                Some((volume, mute)) => (on(*mute), format!("{} %", volume.round())),
                None => (0, String::new()),
            },
            Act::AudioDevice => {
                let target = target_of(inst);
                let current = self.voice[target]["device_id"].as_str().unwrap_or_default();
                (
                    on(
                        find_device(&self.voice, target, &inst.text("device")).as_deref()
                            == Some(current),
                    ),
                    String::new(),
                )
            }
            Act::Camera => (on(self.video), String::new()),
            Act::Screenshare => (on(self.screenshare), String::new()),
            Act::PushToTalk => (on(self.ptt_restore.is_some()), String::new()),
            Act::VoiceMode => (
                on(self.voice["mode"]["type"] == "PUSH_TO_TALK"),
                String::new(),
            ),
            Act::TextChannel | Act::Soundboard => (0, String::new()),
        }
    }

    fn refresh(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let shown = self.appearance(inst);
        if inst.shown.as_ref() == Some(&shown) {
            return;
        }
        let previous = inst.shown.clone();
        if previous.as_ref().map(|p| p.0) != Some(shown.0) {
            self.send(
                json!({ "event": "setState", "context": context, "payload": { "state": shown.0 } }),
            );
        }
        if previous.as_ref().map(|p| p.1.as_str()) != Some(shown.1.as_str()) {
            self.send(
                json!({ "event": "setTitle", "context": context, "payload": { "title": shown.1 } }),
            );
        }
        if let Some(inst) = self.contexts.get_mut(context) {
            inst.shown = Some(shown);
        }
    }

    fn refresh_all(&mut self) {
        let contexts: Vec<String> = self.contexts.keys().cloned().collect();
        for context in contexts {
            self.refresh(&context);
        }
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn on_host(&mut self, msg: Value) {
        let event = msg["event"].as_str().unwrap_or_default();
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match event {
            "didReceiveGlobalSettings" => {
                self.on_global_settings(msg["payload"]["settings"].clone())
            }
            "willAppear" | "didReceiveSettings" => {
                let Some(act) = msg["action"].as_str().and_then(Act::from_uuid) else {
                    return;
                };
                let settings = msg["payload"]["settings"].clone();
                let inst = self.contexts.entry(context.clone()).or_insert(Instance {
                    act,
                    settings: Value::Null,
                    shown: None,
                });
                inst.settings = settings;
                if event == "willAppear" {
                    inst.shown = None;
                }
                self.refresh(&context);
            }
            "willDisappear" => {
                self.contexts.remove(&context);
            }
            "keyDown" => self.on_press(&context, false),
            "dialDown" => self.on_press(&context, true),
            "keyUp" | "dialUp" => self.on_release(&context),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                self.on_rotate(&context, ticks);
            }
            _ => {}
        }
    }

    /// Sends a command in the background; failures flash the key.
    fn call(&self, context: Option<&str>, cmd: &'static str, args: Value) {
        let Some(rpc) = self.rpc.clone() else {
            return;
        };
        let (tx, context) = (self.tx.clone(), context.map(str::to_owned));
        tokio::spawn(async move {
            if let Err(err) = rpc.call(cmd, args).await {
                tx.send(Msg::CallFailed(context, format!("{cmd}: {err:#}")))
                    .ok();
            }
        });
    }

    fn set_voice(&mut self, context: &str, args: Value) {
        // Optimistic: show the result immediately, Discord confirms via event.
        merge(&mut self.voice, &args);
        self.call(Some(context), "SET_VOICE_SETTINGS", args);
        self.refresh_all();
    }

    /// `true` if connected; otherwise flashes the key and tries to connect.
    fn ensure_connected(&mut self, context: &str) -> bool {
        if self.rpc.is_some() {
            return true;
        }
        self.alert(context);
        if !self.creds.complete() {
            self.set_status("Client-ID und Client-Secret eintragen (Plugins → Discord)");
        } else {
            self.start_connect(true);
        }
        false
    }

    fn on_press(&mut self, context: &str, dial: bool) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let act = inst.act;
        if act == Act::Notifications {
            self.notifications = 0;
            self.refresh_all();
            return;
        }
        if !self.ensure_connected(context) {
            return;
        }
        let inst = &self.contexts[context];
        let toggle = |mode: &str, current: bool| match mode {
            "on" => true,
            "off" => false,
            _ => !current,
        };
        match act {
            Act::Mute => {
                let mute = toggle(&inst.mode("toggle"), self.voice_bool("mute"));
                self.set_voice(context, json!({ "mute": mute }));
            }
            Act::Deafen => {
                let deaf = toggle(&inst.mode("toggle"), self.voice_bool("deaf"));
                self.set_voice(context, json!({ "deaf": deaf }));
            }
            Act::PushToTalk => {
                self.ptt_restore = Some(self.voice_bool("mute"));
                self.set_voice(context, json!({ "mute": false }));
            }
            Act::VoiceMode => {
                let ptt = self.voice["mode"]["type"] == "PUSH_TO_TALK";
                let mode = if ptt {
                    "VOICE_ACTIVITY"
                } else {
                    "PUSH_TO_TALK"
                };
                self.set_voice(context, json!({ "mode": { "type": mode } }));
            }
            Act::VoiceChannel => {
                let channel = inst.text("channel");
                if channel.is_empty() {
                    self.alert(context);
                    return;
                }
                let joined = self.channel.as_deref() == Some(channel.as_str());
                let join = match inst.mode("toggle").as_str() {
                    "join" => true,
                    "leave" => false,
                    _ => !joined,
                };
                let args = if join {
                    json!({ "channel_id": channel, "force": true })
                } else {
                    json!({ "channel_id": null })
                };
                self.call(Some(context), "SELECT_VOICE_CHANNEL", args);
            }
            Act::TextChannel => {
                let channel = inst.text("channel");
                if channel.is_empty() {
                    self.alert(context);
                    return;
                }
                self.call(
                    Some(context),
                    "SELECT_TEXT_CHANNEL",
                    json!({ "channel_id": channel }),
                );
            }
            Act::Camera => self.call(Some(context), "TOGGLE_VIDEO", json!({})),
            Act::Screenshare => self.call(Some(context), "TOGGLE_SCREENSHARE", json!({})),
            Act::Soundboard => self.play_sound(context, inst.text("sound")),
            Act::AudioDevice => {
                let target = target_of(inst);
                match find_device(&self.voice, target, &inst.text("device")) {
                    Some(id) => self.set_voice(context, json!({ target: { "device_id": id } })),
                    None => {
                        self.alert(context);
                        self.log(format!("audio device '{}' not found", inst.text("device")));
                    }
                }
            }
            Act::Volume => {
                let target = target_of(inst);
                let mode = if dial {
                    "mute".to_owned()
                } else {
                    inst.mode("mute")
                };
                let step = inst.number("step", 5.0);
                match mode.as_str() {
                    "up" => self.change_volume(context, target, step),
                    "down" => self.change_volume(context, target, -step),
                    _ => {
                        let key = if target == "input" { "mute" } else { "deaf" };
                        let value = !self.voice_bool(key);
                        self.set_voice(context, json!({ key: value }));
                    }
                }
            }
            Act::UserVolume => {
                let user = inst.text("user");
                if user.is_empty() {
                    self.alert(context);
                    return;
                }
                let mode = if dial {
                    "mute".to_owned()
                } else {
                    inst.mode("mute")
                };
                let step = inst.number("step", 10.0);
                match mode.as_str() {
                    "up" => self.change_user_volume(context, &user, step),
                    "down" => self.change_user_volume(context, &user, -step),
                    _ => {
                        let entry = self
                            .user_volumes
                            .entry(user.clone())
                            .or_insert((100.0, false));
                        entry.1 = !entry.1;
                        let mute = entry.1;
                        self.call(
                            Some(context),
                            "SET_USER_VOICE_SETTINGS",
                            json!({ "user_id": user, "mute": mute }),
                        );
                        self.refresh_all();
                    }
                }
            }
            Act::Notifications => {}
        }
    }

    fn on_release(&mut self, context: &str) {
        let is_ptt = self
            .contexts
            .get(context)
            .is_some_and(|i| i.act == Act::PushToTalk);
        if is_ptt && let Some(restore) = self.ptt_restore.take() {
            if restore && self.rpc.is_some() {
                self.set_voice(context, json!({ "mute": true }));
            }
            self.refresh_all();
        }
    }

    fn on_rotate(&mut self, context: &str, ticks: i64) {
        if !self.contexts.contains_key(context) || ticks == 0 {
            return;
        }
        if !self.ensure_connected(context) {
            return;
        }
        let inst = &self.contexts[context];
        match inst.act {
            Act::Volume => {
                let (target, step) = (target_of(inst), inst.number("step", 5.0));
                self.change_volume(context, target, step * ticks as f64);
            }
            Act::UserVolume => {
                let (user, step) = (inst.text("user"), inst.number("step", 10.0));
                if !user.is_empty() {
                    self.change_user_volume(context, &user, step * ticks as f64);
                }
            }
            _ => {}
        }
    }

    fn change_volume(&mut self, context: &str, target: &str, delta: f64) {
        let max = if target == "input" { 100.0 } else { 200.0 };
        let volume = (self.volume_of(target) + delta).clamp(0.0, max).round();
        self.set_voice(context, json!({ target: { "volume": volume } }));
    }

    fn change_user_volume(&mut self, context: &str, user: &str, delta: f64) {
        let entry = self
            .user_volumes
            .entry(user.to_owned())
            .or_insert((100.0, false));
        entry.0 = (entry.0 + delta).clamp(0.0, 200.0).round();
        let volume = entry.0;
        self.call(
            Some(context),
            "SET_USER_VOICE_SETTINGS",
            json!({ "user_id": user, "volume": volume }),
        );
        self.refresh_all();
    }

    fn play_sound(&self, context: &str, wanted: String) {
        let (Some(rpc), tx, context) = (self.rpc.clone(), self.tx.clone(), context.to_owned())
        else {
            return;
        };
        tokio::spawn(async move {
            let result: anyhow::Result<()> = async {
                anyhow::ensure!(!wanted.is_empty(), "kein Sound eingestellt");
                let sounds = rpc.call("GET_SOUNDBOARD_SOUNDS", json!({})).await?;
                let sound = find_sound(&sounds, &wanted)
                    .with_context(|| format!("Sound „{wanted}“ nicht gefunden"))?;
                let mut args = json!({ "sound_id": sound["sound_id"] });
                if let Some(guild) = sound["guild_id"]
                    .as_str()
                    .filter(|g| !g.is_empty() && *g != "0")
                {
                    args["guild_id"] = json!(guild);
                }
                rpc.call("PLAY_SOUNDBOARD_SOUND", args).await?;
                Ok(())
            }
            .await;
            if let Err(err) = result {
                tx.send(Msg::CallFailed(
                    Some(context),
                    format!("soundboard: {err:#}"),
                ))
                .ok();
            }
        });
    }

    fn on_call_failed(&mut self, context: Option<String>, error: String) {
        self.log(error);
        if let Some(context) = context {
            self.alert(&context);
        }
        // Discord reverts on failure; fetch the real state again.
        if let Some(rpc) = self.rpc.clone() {
            let (tx, generation) = (self.tx.clone(), self.generation);
            tokio::spawn(async move {
                if let Ok(voice) = rpc.call("GET_VOICE_SETTINGS", json!({})).await {
                    tx.send(Msg::VoiceSettings(generation, voice)).ok();
                }
            });
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(value) => self.on_host(value),
            Msg::HostClosed => {}
            Msg::Ipc(generation, event) => self.on_ipc(generation, event),
            Msg::Connected(generation, result) => self.on_connected(generation, result),
            Msg::Channel(generation, channel) => self.on_channel(generation, channel),
            Msg::VoiceSettings(generation, voice) => {
                if generation == self.generation {
                    self.voice = voice;
                    self.refresh_all();
                }
            }
            Msg::CallFailed(context, error) => self.on_call_failed(context, error),
        }
    }
}

fn target_of(inst: &Instance) -> &'static str {
    if inst.text("target") == "input" {
        "input"
    } else {
        "output"
    }
}

/// Device id by exact id or (part of the) name, case-insensitive.
fn find_device(voice: &Value, target: &str, wanted: &str) -> Option<String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    let devices = voice[target]["available_devices"].as_array()?;
    let lower = wanted.to_lowercase();
    devices
        .iter()
        .find(|d| d["id"] == wanted)
        .or_else(|| {
            devices.iter().find(|d| {
                d["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase() == lower)
            })
        })
        .or_else(|| {
            devices.iter().find(|d| {
                d["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase().contains(&lower))
            })
        })
        .and_then(|d| d["id"].as_str().map(str::to_owned))
}

fn find_sound<'a>(sounds: &'a Value, wanted: &str) -> Option<&'a Value> {
    let list = sounds.as_array().or_else(|| sounds["sounds"].as_array())?;
    let lower = wanted.to_lowercase();
    list.iter()
        .find(|s| {
            s["sound_id"].as_str() == Some(wanted)
                || s["sound_id"].as_u64().map(|n| n.to_string()).as_deref() == Some(wanted)
        })
        .or_else(|| {
            list.iter().find(|s| {
                s["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase() == lower)
            })
        })
        .or_else(|| {
            list.iter().find(|s| {
                s["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase().contains(&lower))
            })
        })
}

/// Shallow-deep merge for the optimistic voice state.
fn merge(target: &mut Value, patch: &Value) {
    if !target.is_object() {
        *target = json!({});
    }
    if let (Some(t), Some(p)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in p {
            match (t.get_mut(key), value) {
                (Some(existing @ Value::Object(_)), Value::Object(_)) => merge(existing, value),
                _ => {
                    t.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

struct Args {
    port: u16,
    uuid: String,
    register: String,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut port = None;
    let mut uuid = None;
    let mut register = "registerPlugin".to_owned();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-port" => port = it.next().and_then(|p| p.parse().ok()),
            "-pluginUUID" => uuid = it.next(),
            "-registerEvent" => register = it.next().unwrap_or(register),
            _ => {}
        }
    }
    Ok(Args {
        port: port.context("missing -port")?,
        uuid: uuid.context("missing -pluginUUID")?,
        register,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}", args.port))
        .await
        .context("connecting to OpenDeckN3")?;
    let (mut sink, mut stream) = ws.split();

    let (host_tx, mut host_rx) = mpsc::unbounded_channel::<Value>();
    host_tx.send(json!({ "event": args.register, "uuid": args.uuid }))?;
    host_tx.send(json!({ "event": "getGlobalSettings", "context": args.uuid }))?;
    tokio::spawn(async move {
        while let Some(msg) = host_rx.recv().await {
            if sink.send(Message::text(msg.to_string())).await.is_err() {
                break;
            }
        }
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let reader_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Text(text) = msg
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                reader_tx.send(Msg::Host(value)).ok();
            }
        }
        reader_tx.send(Msg::HostClosed).ok();
    });

    let mut plugin = Plugin::new(args.uuid, host_tx, tx);
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                if matches!(msg, Msg::HostClosed) {
                    break;
                }
                plugin.handle(msg);
            }
            _ = tick.tick() => plugin.on_tick(),
        }
    }
    if let Some(rpc) = plugin.rpc.take() {
        rpc.close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn plugin() -> (
        Plugin,
        mpsc::UnboundedReceiver<Value>,
        mpsc::UnboundedReceiver<Msg>,
    ) {
        let (host_tx, host_rx) = mpsc::unbounded_channel();
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Plugin::new("de.opendeckn3.discord".into(), host_tx, tx),
            host_rx,
            rx,
        )
    }

    fn appear(p: &mut Plugin, context: &str, action: &str, settings: Value) {
        p.on_host(json!({ "event": "willAppear", "context": context,
            "action": format!("{PREFIX}{action}"), "payload": { "settings": settings } }));
    }

    async fn connected(p: &mut Plugin, log: Arc<Mutex<Vec<Value>>>) {
        let stream = ipc::tests::fake_discord(move |f| {
            log.lock().unwrap().push(f.clone());
            vec![json!({ "cmd": f["cmd"], "nonce": f["nonce"], "data": {} })]
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        p.rpc = Some(Rpc::handshake(stream, "1", tx).await.unwrap());
        p.voice = json!({
            "mute": false, "deaf": false, "mode": { "type": "VOICE_ACTIVITY" },
            "input": { "volume": 50.0, "device_id": "a", "available_devices": [
                { "id": "a", "name": "Mikrofon (Realtek)" }, { "id": "b", "name": "Headset Mic" }] },
            "output": { "volume": 100.0, "device_id": "x", "available_devices": [] },
        });
    }

    async fn sent(log: &Arc<Mutex<Vec<Value>>>, n: usize) -> Vec<Value> {
        for _ in 0..100 {
            if log.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        log.lock().unwrap().clone()
    }

    #[test]
    fn action_uuids() {
        assert_eq!(
            Act::from_uuid("de.opendeckn3.discord.mute"),
            Some(Act::Mute)
        );
        assert_eq!(
            Act::from_uuid("de.opendeckn3.discord.voicemode"),
            Some(Act::VoiceMode)
        );
        assert_eq!(Act::from_uuid("other.mute"), None);
        // Every action in the manifest is handled.
        let manifest: Value = serde_json::from_str(include_str!(
            "../de.opendeckn3.discord.sdPlugin/manifest.json"
        ))
        .unwrap();
        for action in manifest["Actions"].as_array().unwrap() {
            let uuid = action["UUID"].as_str().unwrap();
            assert!(Act::from_uuid(uuid).is_some(), "{uuid}");
            assert!(!action["States"].as_array().unwrap().is_empty(), "{uuid}");
        }
    }

    #[tokio::test]
    async fn mute_toggles_and_updates_state() {
        let (mut p, mut host, _rx) = plugin();
        let log = Arc::new(Mutex::new(Vec::new()));
        connected(&mut p, log.clone()).await;
        appear(&mut p, "c1", "mute", json!({ "mode": "toggle" }));
        p.on_host(json!({ "event": "keyDown", "context": "c1" }));
        let calls = sent(&log, 1).await;
        assert_eq!(calls[0]["cmd"], "SET_VOICE_SETTINGS");
        assert_eq!(calls[0]["args"], json!({ "mute": true }));
        let mut states = Vec::new();
        while let Ok(m) = host.try_recv() {
            if m["event"] == "setState" {
                states.push(m["payload"]["state"].as_u64().unwrap());
            }
        }
        assert_eq!(states.last(), Some(&1));
        // Discord reports unmute → key follows.
        p.on_ipc(
            p.generation,
            IpcEvent::Dispatch("VOICE_SETTINGS_UPDATE".into(), json!({ "mute": false })),
        );
        let last = std::iter::from_fn(|| host.try_recv().ok())
            .filter(|m| m["event"] == "setState")
            .last();
        assert_eq!(last.unwrap()["payload"]["state"], 0);
    }

    #[tokio::test]
    async fn volume_dial_and_device_switch() {
        let (mut p, mut host, _rx) = plugin();
        let log = Arc::new(Mutex::new(Vec::new()));
        connected(&mut p, log.clone()).await;
        appear(
            &mut p,
            "v",
            "volume",
            json!({ "target": "input", "step": 5 }),
        );
        p.on_host(json!({ "event": "dialRotate", "context": "v", "payload": { "ticks": 3 } }));
        p.on_host(json!({ "event": "dialRotate", "context": "v", "payload": { "ticks": 20 } }));
        appear(
            &mut p,
            "d",
            "audiodevice",
            json!({ "target": "input", "device": "headset" }),
        );
        p.on_host(json!({ "event": "keyDown", "context": "d" }));
        let calls = sent(&log, 3).await;
        assert_eq!(calls[0]["args"], json!({ "input": { "volume": 65.0 } }));
        assert_eq!(
            calls[1]["args"],
            json!({ "input": { "volume": 100.0 } }),
            "clamped to 100"
        );
        assert_eq!(calls[2]["args"], json!({ "input": { "device_id": "b" } }));
        let titles: Vec<String> = std::iter::from_fn(|| host.try_recv().ok())
            .filter(|m| m["event"] == "setTitle" && m["context"] == "v")
            .map(|m| m["payload"]["title"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(titles.last().unwrap(), "100 %");
    }

    #[tokio::test]
    async fn push_to_talk_restores_mute() {
        let (mut p, _host, _rx) = plugin();
        let log = Arc::new(Mutex::new(Vec::new()));
        connected(&mut p, log.clone()).await;
        p.voice["mute"] = json!(true);
        appear(&mut p, "t", "pushtotalk", json!({}));
        p.on_host(json!({ "event": "keyDown", "context": "t" }));
        p.on_host(json!({ "event": "keyUp", "context": "t" }));
        let calls = sent(&log, 2).await;
        assert_eq!(calls[0]["args"], json!({ "mute": false }));
        assert_eq!(calls[1]["args"], json!({ "mute": true }));
    }

    #[tokio::test]
    async fn voice_channel_join_and_leave() {
        let (mut p, _host, _rx) = plugin();
        let log = Arc::new(Mutex::new(Vec::new()));
        connected(&mut p, log.clone()).await;
        appear(&mut p, "ch", "voicechannel", json!({ "channel": "123" }));
        p.on_host(json!({ "event": "keyDown", "context": "ch" }));
        p.channel = Some("123".into());
        p.on_host(json!({ "event": "keyDown", "context": "ch" }));
        let calls = sent(&log, 2).await;
        assert_eq!(calls[0]["cmd"], "SELECT_VOICE_CHANNEL");
        assert_eq!(
            calls[0]["args"],
            json!({ "channel_id": "123", "force": true })
        );
        assert_eq!(calls[1]["args"], json!({ "channel_id": null }));
    }

    #[tokio::test]
    async fn not_connected_alerts_and_asks_for_credentials() {
        let (mut p, mut host, _rx) = plugin();
        p.on_global_settings(json!({}));
        appear(&mut p, "c1", "camera", json!({}));
        p.on_host(json!({ "event": "keyDown", "context": "c1" }));
        let msgs: Vec<Value> = std::iter::from_fn(|| host.try_recv().ok()).collect();
        assert!(msgs.iter().any(|m| m["event"] == "showAlert"));
        let status = msgs
            .iter()
            .rev()
            .find(|m| m["event"] == "setGlobalSettings")
            .unwrap();
        assert!(
            status["payload"]["status"]
                .as_str()
                .unwrap()
                .contains("Client-ID")
        );
    }

    #[test]
    fn sounds_and_devices_by_name() {
        let sounds = json!([{ "sound_id": "1", "name": "Airhorn", "guild_id": "0" }, { "sound_id": "9", "name": "Tada" }]);
        assert_eq!(find_sound(&sounds, "airhorn").unwrap()["sound_id"], "1");
        assert_eq!(find_sound(&sounds, "9").unwrap()["name"], "Tada");
        assert!(find_sound(&sounds, "nope").is_none());
        let voice =
            json!({ "output": { "available_devices": [{ "id": "x", "name": "Lautsprecher" }] } });
        assert_eq!(find_device(&voice, "output", "laut").as_deref(), Some("x"));
        assert_eq!(find_device(&voice, "output", ""), None);
    }
}

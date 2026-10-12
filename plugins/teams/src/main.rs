#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 Teams plugin: controls meetings of the new Microsoft Teams
//! through its local third-party API (see `docs/PLUGIN_TEAMS.md`).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_tungstenite::tungstenite::Message;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const ENDPOINT: &str = "ws://127.0.0.1:8124";

enum Msg {
    Connected(u64),
    Disconnected(u64),
    Received(u64, Value),
}

/// Keeps one connection to Teams alive and forwards its messages; retries every
/// `RECONNECT_DELAY` while Teams is not reachable.
async fn connection(
    url: String,
    mut out: mpsc::UnboundedReceiver<String>,
    tx: mpsc::UnboundedSender<Msg>,
    generation: u64,
) {
    loop {
        if let Ok((ws, _)) = tokio_tungstenite::connect_async(url.as_str()).await {
            tx.send(Msg::Connected(generation)).ok();
            let (mut sink, mut stream) = ws.split();
            loop {
                tokio::select! {
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(value) = serde_json::from_str(&text) {
                                tx.send(Msg::Received(generation, value)).ok();
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    },
                    Some(text) = out.recv() => {
                        if sink.send(Message::text(text)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
        tx.send(Msg::Disconnected(generation)).ok();
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Meeting state as Teams reports it in `meetingUpdate`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Meeting {
    in_meeting: bool,
    muted: bool,
    video: bool,
    hand: bool,
    blur: bool,
    permissions: Map<String, Value>,
}

impl Meeting {
    /// Takes the value of `meetingUpdate`; missing fields count as off.
    fn parse(update: &Value) -> Self {
        let state = &update["meetingState"];
        let flag = |key: &str| state[key] == true;
        Self {
            in_meeting: flag("isInMeeting"),
            muted: flag("isMuted"),
            video: flag("isVideoOn"),
            hand: flag("isHandRaised"),
            blur: flag("isBackgroundBlurred"),
            permissions: update["meetingPermissions"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Only an explicit `false` forbids; a permission Teams does not report is allowed.
    fn permitted(&self, permission: &str) -> bool {
        self.permissions.get(permission) != Some(&Value::Bool(false))
    }
}

/// One Teams command: the action name, its parameters and the permission it needs.
#[derive(Debug, PartialEq)]
struct Command {
    action: &'static str,
    parameters: Value,
    permission: &'static str,
}

/// Maps a key kind to the command it sends; `reaction` is used by `react` only.
fn command(kind: &str, reaction: &str) -> Option<Command> {
    let (action, parameters, permission) = match kind {
        "mute" => ("toggle-mute", json!({}), "canToggleMute"),
        "video" => ("toggle-video", json!({}), "canToggleVideo"),
        "hand" => ("toggle-hand", json!({}), "canToggleHand"),
        "blur" => ("toggle-background-blur", json!({}), "canToggleBlur"),
        "react" => ("send-reaction", json!({ "type": reaction }), "canReact"),
        "leave" => ("leave-call", json!({}), "canLeave"),
        _ => return None,
    };
    Some(Command {
        action,
        parameters,
        permission,
    })
}

/// Whether the key's state image is lit, i.e. the toggle is currently on.
fn lit(meeting: &Meeting, kind: &str) -> bool {
    match kind {
        "mute" => meeting.muted,
        "video" => meeting.video,
        "hand" => meeting.hand,
        "blur" => meeting.blur,
        _ => false,
    }
}

/// Local endpoint with the pairing token, if one is stored. Values are
/// URL-encoded by the `url` crate.
fn endpoint(token: &str) -> String {
    let mut params = vec![
        ("protocol-version", "2.0.0"),
        ("manufacturer", "OpenDeckN3"),
        ("device", "N3"),
        ("app", "OpenDeckN3"),
        ("app-version", env!("CARGO_PKG_VERSION")),
    ];
    if !token.is_empty() {
        params.insert(0, ("token", token));
    }
    url::Url::parse_with_params(ENDPOINT, params)
        .expect("constant endpoint is a valid URL")
        .into()
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    /// Incremented on every restart; messages of older connections are ignored.
    generation: u64,
    task: Option<JoinHandle<()>>,
    out: Option<mpsc::UnboundedSender<String>>,
    next_id: u64,
    connected: bool,
    meeting: Meeting,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            global: Map::new(),
            generation: 0,
            task: None,
            out: None,
            next_id: 0,
            connected: false,
            meeting: Meeting::default(),
        }
    }

    fn set_status(&mut self, status: &str) {
        if self.global.get("status").and_then(Value::as_str) != Some(status) {
            self.global.insert("status".into(), json!(status));
            self.host.save_global(&self.global);
        }
    }

    fn on_global(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        // The status write-back echoes the settings; only a new token changes the endpoint.
        if self.generation == 0 || previous.get("token") != self.global.get("token") {
            self.restart();
        }
    }

    fn restart(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation += 1;
        self.out = None;
        self.connected = false;
        self.meeting = Meeting::default();
        if cfg!(windows) {
            let (out, out_rx) = mpsc::unbounded_channel();
            let url = endpoint(&global_str(&self.global, "token"));
            self.task = Some(tokio::spawn(connection(
                url,
                out_rx,
                self.tx.clone(),
                self.generation,
            )));
            self.out = Some(out);
            self.set_status("Verbinde mit Teams …");
        } else {
            self.set_status("Nur für Windows");
        }
        self.refresh_all();
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Connected(generation) if generation == self.generation => {
                self.connected = true;
                self.set_status("Verbunden mit Teams");
                self.refresh_all();
            }
            Msg::Disconnected(generation) if generation == self.generation => {
                self.connected = false;
                self.meeting = Meeting::default();
                self.set_status("Teams nicht erreichbar – neuer Versuch alle 5 s");
                self.refresh_all();
            }
            Msg::Received(generation, value) if generation == self.generation => {
                self.on_teams(&value)
            }
            _ => {}
        }
    }

    /// Meeting updates change the key states; a token is stored for the next start.
    fn on_teams(&mut self, value: &Value) {
        if let Some(update) = value.get("meetingUpdate") {
            self.meeting = Meeting::parse(update);
            self.refresh_all();
        } else if let Some(token) = value["tokenRefresh"].as_str().filter(|t| !t.is_empty()) {
            self.global.insert("token".into(), json!(token));
            self.host.save_global(&self.global);
        }
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        let title = if self.connected && self.meeting.in_meeting {
            ""
        } else {
            "–"
        };
        let state = u16::from(self.meeting.in_meeting && lit(&self.meeting, inst.kind()));
        (state, title.to_owned())
    }

    fn refresh(&mut self, context: &str) {
        if let Some(inst) = self.contexts.get(context) {
            let (state, title) = self.appearance(inst);
            self.contexts.show(&self.host, context, state, &title);
        }
    }

    fn refresh_all(&mut self) {
        for context in self.contexts.ids() {
            self.refresh(&context);
        }
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.refresh(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "keyDown" => self.press(&context),
            _ => {}
        }
    }

    fn press(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let Some(command) = command(inst.kind(), &inst.text_or("reaction", "like")) else {
            return;
        };
        if !self.meeting.in_meeting || !self.meeting.permitted(command.permission) {
            self.host.alert(context);
            return;
        }
        self.next_id += 1;
        let request = json!({
            "action": command.action,
            "parameters": command.parameters,
            "requestId": self.next_id,
        });
        if let Some(out) = &self.out {
            out.send(request.to_string()).ok();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.on_msg(msg),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Teams message carrying a `meetingUpdate`.
    fn update(muted: bool, in_meeting: bool, permissions: Value) -> Value {
        json!({ "meetingUpdate": {
            "meetingState": { "isInMeeting": in_meeting, "isMuted": muted, "isVideoOn": true,
                              "isHandRaised": false, "isBackgroundBlurred": false },
            "meetingPermissions": permissions,
        } })
    }

    fn key(context: &str, kind: &str, settings: Value) -> Value {
        json!({ "event": "willAppear", "context": context, "action": format!("de.opendeckn3.teams.{kind}"),
            "payload": { "controller": "Keypad", "settings": settings } })
    }

    #[test]
    fn parses_meeting_state_and_permissions() {
        let meeting =
            Meeting::parse(&update(true, true, json!({ "canToggleMute": false }))["meetingUpdate"]);
        assert!(meeting.in_meeting && meeting.muted && meeting.video);
        assert!(!meeting.hand && !meeting.blur);
        assert!(!meeting.permitted("canToggleMute"));
        assert!(meeting.permitted("canToggleVideo"));

        let empty = Meeting::parse(&json!({}));
        assert_eq!(empty, Meeting::default());
    }

    #[test]
    fn maps_keys_to_commands() {
        assert_eq!(
            command("mute", "").unwrap(),
            Command {
                action: "toggle-mute",
                parameters: json!({}),
                permission: "canToggleMute",
            }
        );
        assert_eq!(
            command("react", "applause").unwrap().parameters,
            json!({ "type": "applause" })
        );
        assert_eq!(command("leave", "").unwrap().action, "leave-call");
        assert!(command("unbekannt", "").is_none());
    }

    #[test]
    fn builds_endpoint_with_optional_token() {
        let without = endpoint("");
        assert!(!without.contains("token="));
        assert!(
            without
                .starts_with("ws://127.0.0.1:8124/?protocol-version=2.0.0&manufacturer=OpenDeckN3")
        );
        assert!(without.contains(&format!("app-version={}", env!("CARGO_PKG_VERSION"))));

        let with = endpoint("a b&c");
        assert!(
            with.contains("token=a+b%26c&protocol-version=2.0.0"),
            "{with}"
        );
    }

    #[test]
    fn token_refresh_is_saved_and_unknown_messages_are_ignored() {
        let (host, mut rx) = Host::channel("de.opendeckn3.teams");
        let (tx, _msgs) = mpsc::unbounded_channel();
        let mut plugin = Plugin::new(host, tx);
        plugin.on_teams(&json!({ "somethingElse": 1 }));
        assert!(rx.try_recv().is_err());

        plugin.on_teams(&json!({ "tokenRefresh": "geheim" }));
        let saved = rx.try_recv().unwrap();
        assert_eq!(saved["event"], "setGlobalSettings");
        assert_eq!(saved["payload"]["token"], "geheim");
    }

    #[test]
    fn keys_show_meeting_state_and_press_respects_permissions() {
        let (host, mut host_rx) = Host::channel("de.opendeckn3.teams");
        let (tx, _msgs) = mpsc::unbounded_channel();
        let mut plugin = Plugin::new(host, tx);
        let (out, mut out_rx) = mpsc::unbounded_channel();
        plugin.out = Some(out);
        plugin.connected = true;
        plugin.contexts.apply(&key("m", "mute", json!({})));
        plugin.contexts.apply(&key("v", "video", json!({})));

        plugin.on_teams(&update(true, false, json!({})));
        while host_rx.try_recv().is_ok() {}
        plugin.press("m");
        let events: Vec<Value> = std::iter::from_fn(|| host_rx.try_recv().ok()).collect();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["event"], "showAlert");
        assert_eq!(events[0]["context"], "m");
        assert!(out_rx.try_recv().is_err());

        plugin.on_teams(&update(true, true, json!({ "canToggleMute": false })));
        let appearance = |context: &str| plugin.appearance(plugin.contexts.get(context).unwrap());
        assert_eq!(appearance("m"), (1, String::new()));
        assert_eq!(appearance("v"), (1, String::new()));
        plugin.press("m");
        plugin.press("v");
        let sent = out_rx.try_recv().unwrap();
        assert_eq!(
            sent,
            json!({ "action": "toggle-video", "parameters": {}, "requestId": 1 }).to_string()
        );
        let alerts: Vec<Value> = std::iter::from_fn(|| host_rx.try_recv().ok())
            .filter(|e| e["event"] == "showAlert")
            .collect();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0]["context"], "m");
        assert!(out_rx.try_recv().is_err());
    }
}

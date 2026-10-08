//! Helpers for OpenDeckN3 plugins written in Rust.
//!
//! [`connect`] parses the launch arguments, connects to the host and
//! registers. [`Host`] sends events back; [`Contexts`] tracks the keys and
//! knobs an action is placed on and only sends state/title changes.

use std::collections::HashMap;

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Sends events to OpenDeckN3. Cheap to clone.
#[derive(Clone)]
pub struct Host {
    uuid: String,
    tx: mpsc::UnboundedSender<Value>,
}

impl Host {
    /// A host that writes into a channel (for tests).
    pub fn channel(uuid: &str) -> (Self, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                uuid: uuid.to_owned(),
                tx,
            },
            rx,
        )
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn send(&self, event: Value) {
        self.tx.send(event).ok();
    }

    pub fn set_state(&self, context: &str, state: u16) {
        self.send(
            json!({ "event": "setState", "context": context, "payload": { "state": state } }),
        );
    }

    pub fn set_title(&self, context: &str, title: &str) {
        self.send(
            json!({ "event": "setTitle", "context": context, "payload": { "title": title } }),
        );
    }

    pub fn alert(&self, context: &str) {
        self.send(json!({ "event": "showAlert", "context": context }));
    }

    pub fn ok(&self, context: &str) {
        self.send(json!({ "event": "showOk", "context": context }));
    }

    pub fn log(&self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("{}: {message}", self.uuid);
        self.send(json!({ "event": "logMessage", "payload": { "message": message } }));
    }

    /// Replaces the plugin's global settings.
    pub fn save_global(&self, settings: &Map<String, Value>) {
        self.send(json!({
            "event": "setGlobalSettings",
            "context": self.uuid,
            "payload": Value::Object(settings.clone()),
        }));
    }

    /// Answers a `sendToPlugin` request of the app (settings dropdowns).
    pub fn reply_options(&self, request: &Value, options: anyhow::Result<Vec<(String, String)>>) {
        let id = request["requestId"].clone();
        let payload = match options {
            Ok(options) => json!({ "requestId": id, "options": options }),
            Err(err) => json!({ "requestId": id, "error": format!("{err:#}") }),
        };
        self.send(
            json!({ "event": "sendToPropertyInspector", "context": self.uuid, "payload": payload }),
        );
    }
}

/// One placed action.
#[derive(Clone, Debug)]
pub struct Instance {
    pub action: String,
    pub controller: String,
    pub settings: Value,
    shown: Option<(u16, String)>,
}

impl Instance {
    /// Action UUID without the plugin prefix, e.g. `record`.
    pub fn kind(&self) -> &str {
        self.action.rsplit('.').next().unwrap_or_default()
    }

    pub fn is_encoder(&self) -> bool {
        self.controller == "Encoder"
    }

    /// Setting as trimmed text (numbers are formatted).
    pub fn text(&self, key: &str) -> String {
        match &self.settings[key] {
            Value::String(s) => s.trim().to_owned(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    pub fn text_or(&self, key: &str, default: &str) -> String {
        Some(self.text(key))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| default.to_owned())
    }

    pub fn number(&self, key: &str, default: f64) -> f64 {
        match &self.settings[key] {
            Value::Number(n) => n.as_f64().unwrap_or(default),
            Value::String(s) => s.trim().replace(',', ".").parse().unwrap_or(default),
            _ => default,
        }
    }

    /// `"yes"`/`"true"`/`true` → true.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        match &self.settings[key] {
            Value::Bool(b) => *b,
            Value::String(s) => matches!(s.as_str(), "yes" | "true" | "on" | "1"),
            _ => default,
        }
    }
}

/// Keys/knobs the plugin's actions are placed on.
#[derive(Default)]
pub struct Contexts {
    map: HashMap<String, Instance>,
}

impl Contexts {
    /// Updates the table from a host message. Returns the context when an
    /// instance appeared or its settings changed (caller should refresh it).
    pub fn apply(&mut self, msg: &Value) -> Option<String> {
        let context = msg["context"].as_str()?.to_owned();
        match msg["event"].as_str()? {
            "willAppear" | "didReceiveSettings" => {
                let payload = &msg["payload"];
                let inst = self.map.entry(context.clone()).or_insert_with(|| Instance {
                    action: msg["action"].as_str().unwrap_or_default().to_owned(),
                    controller: payload["controller"]
                        .as_str()
                        .unwrap_or("Keypad")
                        .to_owned(),
                    settings: Value::Null,
                    shown: None,
                });
                inst.settings = payload["settings"].clone();
                if msg["event"] == "willAppear" {
                    inst.shown = None;
                }
                Some(context)
            }
            "willDisappear" => {
                self.map.remove(&context);
                None
            }
            _ => None,
        }
    }

    pub fn get(&self, context: &str) -> Option<&Instance> {
        self.map.get(context)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Instance)> {
        self.map.iter()
    }

    pub fn ids(&self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }

    pub fn any(&self, kind: &str) -> bool {
        self.map.values().any(|i| i.kind() == kind)
    }

    /// Sends state and title only if they changed.
    pub fn show(&mut self, host: &Host, context: &str, state: u16, title: &str) {
        let Some(inst) = self.map.get_mut(context) else {
            return;
        };
        let previous = inst.shown.replace((state, title.to_owned()));
        if previous.as_ref().map(|p| p.0) != Some(state) {
            host.set_state(context, state);
        }
        if previous.as_ref().map(|p| p.1.as_str()) != Some(title) {
            host.set_title(context, title);
        }
    }
}

/// Connects to OpenDeckN3 using the launch arguments (`-port`,
/// `-pluginUUID`, `-registerEvent`), registers and asks for the global
/// settings. Returns the host handle and the stream of host messages; the
/// stream ends when OpenDeckN3 closes the connection.
pub async fn connect() -> anyhow::Result<(Host, mpsc::UnboundedReceiver<Value>)> {
    let mut port = None;
    let mut uuid = None;
    let mut register = "registerPlugin".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-port" => port = args.next().and_then(|p| p.parse::<u16>().ok()),
            "-pluginUUID" => uuid = args.next(),
            "-registerEvent" => register = args.next().unwrap_or(register),
            _ => {}
        }
    }
    let port = port.context("missing -port")?;
    let uuid = uuid.context("missing -pluginUUID")?;

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
        .await
        .context("connecting to OpenDeckN3")?;
    let (mut sink, mut stream) = ws.split();
    let (tx, mut out) = mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        while let Some(msg) = out.recv().await {
            if sink.send(Message::text(msg.to_string())).await.is_err() {
                break;
            }
        }
    });
    let host = Host { uuid, tx };
    host.send(json!({ "event": register, "uuid": host.uuid }));
    host.send(json!({ "event": "getGlobalSettings", "context": host.uuid }));

    let (in_tx, in_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Text(text) = msg
                && let Ok(value) = serde_json::from_str::<Value>(&text)
                && in_tx.send(value).is_err()
            {
                break;
            }
        }
    });
    Ok((host, in_rx))
}

/// Text of a global setting.
pub fn global_str(global: &Map<String, Value>, key: &str) -> String {
    global
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_track_and_dedupe() {
        let (host, mut rx) = Host::channel("p");
        let mut ctx = Contexts::default();
        let appeared = ctx.apply(&json!({ "event": "willAppear", "context": "c", "action": "p.record",
            "payload": { "controller": "Encoder", "settings": { "step": "2,5", "timer": "yes" } } }));
        assert_eq!(appeared.as_deref(), Some("c"));
        let inst = ctx.get("c").unwrap();
        assert_eq!(inst.kind(), "record");
        assert!(inst.is_encoder());
        assert_eq!(inst.number("step", 1.0), 2.5);
        assert!(inst.flag("timer", false));
        ctx.show(&host, "c", 1, "REC");
        ctx.show(&host, "c", 1, "REC");
        ctx.show(&host, "c", 1, "00:01");
        let sent: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(
            sent.len(),
            3,
            "state + title, then only the new title: {sent:?}"
        );
        ctx.apply(&json!({ "event": "willDisappear", "context": "c" }));
        assert!(ctx.get("c").is_none());
    }
}

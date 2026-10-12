#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 MQTT plugin: publishes messages per key or dial, shows the
//! last value of a topic and switches devices via command/state topics
//! (see `docs/PLUGIN_MQTT.md`).

use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

use anyhow::Context;
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, Transport};
use serde_json::{Map, Value, json};
use tokio::{sync::mpsc, task::JoinHandle};

const TICK: Duration = Duration::from_secs(1);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const REQUEST_CAPACITY: usize = 10;
const DEFAULT_PORT: u16 = 1883;
const ERROR_TITLE: &str = "–";

enum Msg {
    Connected(u64),
    Disconnected(u64, String),
    Received(u64, String, String),
}

/// Runs the event loop of one connection; errors end in a retry after a delay.
async fn connection(mut eventloop: EventLoop, tx: mpsc::UnboundedSender<Msg>, generation: u64) {
    loop {
        let msg = match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => Msg::Connected(generation),
            Ok(Event::Incoming(Packet::Publish(publish))) => Msg::Received(
                generation,
                publish.topic,
                String::from_utf8_lossy(&publish.payload).into_owned(),
            ),
            Ok(_) => continue,
            Err(err) => {
                tx.send(Msg::Disconnected(generation, err.to_string())).ok();
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        tx.send(msg).ok();
    }
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    /// Incremented on every restart; messages of older connections are ignored.
    generation: u64,
    task: Option<JoinHandle<()>>,
    client: Option<AsyncClient>,
    label: String,
    connected: bool,
    /// Topics subscribed on the current connection.
    active: BTreeSet<String>,
    /// Last raw payload per subscribed topic.
    values: HashMap<String, String>,
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
            client: None,
            label: String::new(),
            connected: false,
            active: BTreeSet::new(),
            values: HashMap::new(),
        }
    }

    fn set_status(&mut self, status: impl Into<String>) {
        let status = status.into();
        if self.global.get("status").and_then(Value::as_str) != Some(status.as_str()) {
            self.global.insert("status".into(), json!(status));
            self.host.save_global(&self.global);
        }
    }

    fn on_global(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        let changed = ["host", "port", "username", "password", "tls"]
            .into_iter()
            .any(|key| previous.get(key) != self.global.get(key));
        // The status write-back echoes the settings; only broker changes restart the connection.
        if self.generation == 0 || changed {
            self.restart();
        }
    }

    fn restart(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation += 1;
        self.client = None;
        self.connected = false;
        self.active.clear();
        self.values.clear();
        match broker_options(&self.global) {
            Ok(Some(opts)) => {
                let (host, port) = opts.broker_address();
                self.label = format!("{host}:{port}");
                let (client, eventloop) = AsyncClient::new(opts, REQUEST_CAPACITY);
                self.client = Some(client);
                self.task = Some(tokio::spawn(connection(
                    eventloop,
                    self.tx.clone(),
                    self.generation,
                )));
                self.set_status(format!("Verbinde mit {} …", self.label));
            }
            Ok(None) => self.set_status("Kein Broker eingetragen"),
            Err(err) => self.set_status(format!("{err:#}")),
        }
        self.refresh_all();
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Connected(generation) if generation == self.generation => {
                self.connected = true;
                self.set_status(format!("Verbunden mit {}", self.label));
                self.sync_subscriptions();
                self.refresh_all();
            }
            Msg::Disconnected(generation, err) if generation == self.generation => {
                self.connected = false;
                self.active.clear();
                self.set_status(format!(
                    "Nicht verbunden: {err}, neuer Versuch in {} s",
                    RECONNECT_DELAY.as_secs()
                ));
                self.refresh_all();
            }
            Msg::Received(generation, topic, payload) if generation == self.generation => {
                if self.active.contains(&topic) {
                    self.values.insert(topic, payload);
                }
                self.refresh_all();
            }
            Msg::Connected(_) | Msg::Disconnected(..) | Msg::Received(..) => {}
        }
    }

    /// Subscribes the topics the visible keys need and drops the others.
    /// Runs on every tick, so a subscription that could not be queued is retried.
    fn sync_subscriptions(&mut self) {
        if !self.connected {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let (subscribe, unsubscribe) =
            subscription_changes(&self.active, &wanted_topics(&self.contexts));
        for topic in subscribe {
            if client
                .try_subscribe(topic.as_str(), QoS::AtMostOnce)
                .is_ok()
            {
                self.active.insert(topic);
            }
        }
        for topic in unsubscribe {
            if client.try_unsubscribe(topic.as_str()).is_ok() {
                self.active.remove(&topic);
                self.values.remove(&topic);
            }
        }
    }

    fn is_on(&self, inst: &Instance) -> bool {
        self.values
            .get(&inst.text("stateTopic"))
            .is_some_and(|raw| {
                switch_on(raw, &inst.text("pointer"), &inst.text_or("payloadOn", "ON"))
            })
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        match inst.kind() {
            "switch" if self.connected => (u16::from(self.is_on(inst)), String::new()),
            "value" if self.connected => (0, self.value_title(inst)),
            "switch" | "value" => (0, "offline".into()),
            _ => (0, String::new()),
        }
    }

    fn value_title(&self, inst: &Instance) -> String {
        match self.values.get(&inst.text("topic")) {
            None => String::new(),
            Some(raw) => payload_value(raw, &inst.text("pointer")).map_or_else(
                |_| ERROR_TITLE.to_owned(),
                |value| format!("{value}{}", inst.text("suffix")),
            ),
        }
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
        let applied = self.contexts.apply(&msg);
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => self.rotate(&context, msg["payload"]["ticks"].as_i64().unwrap_or(0)),
            _ => {}
        }
        self.sync_subscriptions();
        if let Some(context) = applied {
            self.refresh(&context);
        }
    }

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        match inst.kind() {
            "publish" => self.publish(context, inst, 0),
            "switch" => self.toggle(context, inst),
            _ => {}
        }
    }

    fn rotate(&self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        if inst.kind() == "publish" && ticks != 0 {
            self.publish(context, inst, ticks);
        }
    }

    fn publish(&self, context: &str, inst: &Instance, ticks: i64) {
        let payload = fill_ticks(inst.settings["payload"].as_str().unwrap_or_default(), ticks);
        let qos = if inst.text_or("qos", "0") == "1" {
            QoS::AtLeastOnce
        } else {
            QoS::AtMostOnce
        };
        self.send(
            context,
            &inst.text("topic"),
            payload,
            qos,
            inst.flag("retain", false),
        );
    }

    fn toggle(&self, context: &str, inst: &Instance) {
        let payload = if self.is_on(inst) {
            inst.text_or("payloadOff", "OFF")
        } else {
            inst.text_or("payloadOn", "ON")
        };
        self.send(
            context,
            &inst.text("commandTopic"),
            payload,
            QoS::AtMostOnce,
            false,
        );
    }

    fn send(&self, context: &str, topic: &str, payload: String, qos: QoS, retain: bool) {
        match self.try_send(topic, payload, qos, retain) {
            Ok(()) => self.host.ok(context),
            Err(err) => {
                self.host.alert(context);
                self.host
                    .log(format!("MQTT-Nachricht nicht gesendet: {err:#}"));
            }
        }
    }

    fn try_send(&self, topic: &str, payload: String, qos: QoS, retain: bool) -> anyhow::Result<()> {
        check_publish_topic(topic)?;
        let client = self
            .client
            .as_ref()
            .filter(|_| self.connected)
            .context("nicht mit dem Broker verbunden")?;
        client
            .try_publish(topic, qos, retain, payload)
            .context("Verbindung ausgelastet")
    }
}

/// Broker settings from the global settings; `None` while no host is entered.
fn broker_options(global: &Map<String, Value>) -> anyhow::Result<Option<MqttOptions>> {
    let host = global_str(global, "host");
    if host.is_empty() {
        return Ok(None);
    }
    let port = match global.get("port").filter(|v| !v.is_null()) {
        Some(value) => value
            .as_u64()
            .or_else(|| value.as_str()?.trim().parse().ok())
            .and_then(|port| u16::try_from(port).ok())
            .context("Port muss zwischen 1 und 65535 liegen")?,
        None => DEFAULT_PORT,
    };
    let mut opts = MqttOptions::new(format!("opendeckn3-{}", std::process::id()), host, port);
    let username = global_str(global, "username");
    if !username.is_empty() {
        // Not trimmed: a password may begin or end with spaces.
        let password = global
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or_default();
        opts.set_credentials(username, password);
    }
    if global_str(global, "tls") == "yes" {
        // rumqttc is built without a crypto provider; ring is the one the other plugins use.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        opts.set_transport(Transport::tls_with_default_config());
    }
    Ok(Some(opts))
}

/// Topics that visible keys need: value keys show `topic`, switches read `stateTopic`.
fn wanted_topics(contexts: &Contexts) -> BTreeSet<String> {
    contexts
        .iter()
        .filter_map(|(_, inst)| match inst.kind() {
            "value" => Some(inst.text("topic")),
            "switch" => Some(inst.text("stateTopic")),
            _ => None,
        })
        .filter(|topic| !topic.is_empty())
        .collect()
}

/// (topics to subscribe, topics to unsubscribe)
fn subscription_changes(
    active: &BTreeSet<String>,
    wanted: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    (
        wanted.difference(active).cloned().collect(),
        active.difference(wanted).cloned().collect(),
    )
}

/// Wildcards are only valid in subscriptions; the broker drops the connection on a publish to them.
fn check_publish_topic(topic: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!topic.is_empty(), "Topic fehlt");
    anyhow::ensure!(
        !topic.contains(['+', '#']),
        "Topic darf keine Platzhalter + oder # enthalten"
    );
    Ok(())
}

/// `{{ticks}}` becomes the signed step count, e.g. `2` or `-1`; no `+`, which
/// a query string would read as a space.
fn fill_ticks(text: &str, ticks: i64) -> String {
    text.replace("{{ticks}}", &ticks.to_string())
}

/// Value of a message: the whole text, or the JSON value at `pointer`.
/// Text that is not JSON is shown as it is.
fn payload_value(payload: &str, pointer: &str) -> anyhow::Result<String> {
    let raw = payload.trim();
    if pointer.is_empty() {
        return Ok(raw.to_owned());
    }
    let Ok(json) = serde_json::from_str::<Value>(raw) else {
        return Ok(raw.to_owned());
    };
    let value = json
        .pointer(pointer)
        .with_context(|| format!("{pointer} fehlt in der Nachricht"))?;
    Ok(match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn switch_on(payload: &str, pointer: &str, payload_on: &str) -> bool {
    payload_value(payload, pointer).is_ok_and(|value| value == payload_on)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx);
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.on_msg(msg),
            _ = tick.tick() => plugin.sync_subscriptions(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contexts(instances: &[Value]) -> Contexts {
        let mut contexts = Contexts::default();
        for inst in instances {
            contexts.apply(inst);
        }
        contexts
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn payload_text_and_pointer() {
        assert_eq!(payload_value(" 21.5\n", "").unwrap(), "21.5");
        assert_eq!(payload_value(r#"{"state":"ON"}"#, "/state").unwrap(), "ON");
        assert_eq!(payload_value(r#"{"t":21.5}"#, "/t").unwrap(), "21.5");
        assert_eq!(
            payload_value(r#"{"o":{"x":1}}"#, "/o").unwrap(),
            r#"{"x":1}"#
        );
        assert_eq!(payload_value("kein json", "/state").unwrap(), "kein json");
        assert!(payload_value(r#"{"a":1}"#, "/fehlt").is_err());
    }

    #[test]
    fn switch_state_from_payload() {
        assert!(switch_on(r#"{"state":"ON"}"#, "/state", "ON"));
        assert!(!switch_on(r#"{"state":"OFF"}"#, "/state", "ON"));
        assert!(switch_on("ON", "", "ON"));
        assert!(!switch_on("OFF", "", "ON"));
        assert!(switch_on("1", "", "1"));
        assert!(!switch_on(r#"{"x":1}"#, "/state", "ON"));
    }

    #[test]
    fn wanted_topics_are_unique() {
        let contexts = contexts(&[
            json!({ "event": "willAppear", "context": "a", "action": "p.value",
                "payload": { "controller": "Keypad", "settings": { "topic": "z/temp" } } }),
            json!({ "event": "willAppear", "context": "b", "action": "p.value",
                "payload": { "controller": "Keypad", "settings": { "topic": "z/temp" } } }),
            json!({ "event": "willAppear", "context": "c", "action": "p.switch",
                "payload": { "controller": "Keypad", "settings": { "stateTopic": "z/lamp", "commandTopic": "z/lamp/set" } } }),
            json!({ "event": "willAppear", "context": "d", "action": "p.publish",
                "payload": { "controller": "Keypad", "settings": { "topic": "z/other" } } }),
            json!({ "event": "willAppear", "context": "e", "action": "p.value",
                "payload": { "controller": "Keypad", "settings": {} } }),
        ]);
        assert_eq!(wanted_topics(&contexts), set(&["z/lamp", "z/temp"]));
    }

    #[test]
    fn subscription_diff() {
        let (sub, unsub) = subscription_changes(&set(&["a", "b"]), &set(&["b", "c"]));
        assert_eq!(sub, vec!["c".to_owned()]);
        assert_eq!(unsub, vec!["a".to_owned()]);

        let (sub, unsub) = subscription_changes(&set(&["a"]), &set(&["a"]));
        assert!(sub.is_empty() && unsub.is_empty());
    }

    #[test]
    fn publish_topic_and_ticks() {
        assert!(check_publish_topic("zigbee2mqtt/Lampe/set/state").is_ok());
        assert!(check_publish_topic("").is_err());
        assert!(check_publish_topic("zigbee2mqtt/+/set").is_err());
        assert!(check_publish_topic("home/#").is_err());
        assert_eq!(
            fill_ticks("{\"brightness\": {{ticks}}}", -3),
            "{\"brightness\": -3}"
        );
        assert_eq!(fill_ticks("ohne Platzhalter", 1), "ohne Platzhalter");
    }

    #[test]
    fn broker_settings() {
        assert!(broker_options(&Map::new()).unwrap().is_none());
        let global = json!({ "host": "192.168.1.10", "port": 1884, "username": "n3", "password": " x ", "tls": "yes" });
        let opts = broker_options(global.as_object().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(opts.broker_address(), ("192.168.1.10".to_owned(), 1884));
        assert_eq!(opts.credentials().unwrap().password, " x ");
        let bad = json!({ "host": "h", "port": "abc" });
        assert!(broker_options(bad.as_object().unwrap()).is_err());
        let default_port = json!({ "host": "h" });
        let opts = broker_options(default_port.as_object().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(opts.broker_address().1, DEFAULT_PORT);
    }
}

#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 VoiceMeeter plugin: levels, mutes, bus routing and parameter
//! scripts of VoiceMeeter Standard, Banana and Potato through the official
//! Remote API (see `docs/PLUGIN_VOICEMEETER.md`). Windows only.

// The DLL side exists only on Windows; elsewhere the shared types are built but never used.
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[cfg(not(windows))]
mod platform {
    use tokio::sync::mpsc::UnboundedSender;

    use crate::{Command, Event};

    const UNSUPPORTED: &str = "VoiceMeeter gibt es nur für Windows";

    pub fn start(events: UnboundedSender<Event>) {
        events.send(Event::Offline(UNSUPPORTED.into())).ok();
    }

    pub async fn run(_: Command) -> anyhow::Result<()> {
        anyhow::bail!(UNSUPPORTED)
    }
}

use std::collections::HashMap;

use anyhow::{Context, anyhow};
use n3_plugin_sdk::{Contexts, Host, Instance};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const DEFAULT_STEP: f64 = 1.0;
const MIN_DB: f32 = -60.0;
const MAX_DB: f32 = 12.0;
const ERROR_TITLE: &str = "–";
const NOT_CONNECTED: &str = "VoiceMeeter ist nicht verbunden";

/// Strip and bus counts of one VoiceMeeter edition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub name: &'static str,
    pub strips: usize,
    pub buses_a: usize,
    pub buses_b: usize,
}

impl Layout {
    /// The value of `VBVMR_GetVoicemeeterType`: 1 Standard, 2 Banana, 3 Potato, 6 Potato x64.
    pub fn from_type(kind: i64) -> Option<Self> {
        Some(match kind {
            1 => Self {
                name: "VoiceMeeter",
                strips: 3,
                buses_a: 1,
                buses_b: 1,
            },
            2 => Self {
                name: "VoiceMeeter Banana",
                strips: 5,
                buses_a: 3,
                buses_b: 2,
            },
            3 | 6 => Self {
                name: "VoiceMeeter Potato",
                strips: 8,
                buses_a: 5,
                buses_b: 3,
            },
            _ => return None,
        })
    }

    pub fn strip_names(&self) -> Vec<String> {
        (0..self.strips).map(|i| format!("Strip[{i}]")).collect()
    }

    pub fn bus_names(&self) -> Vec<String> {
        (0..self.buses_a + self.buses_b)
            .map(|i| format!("Bus[{i}]"))
            .collect()
    }

    /// Output labels in bus index order: A1…, then B1….
    pub fn bus_labels(&self) -> Vec<String> {
        (1..=self.buses_a)
            .map(|n| format!("A{n}"))
            .chain((1..=self.buses_b).map(|n| format!("B{n}")))
            .collect()
    }

    /// Every parameter the plugin reads: gain and mute of strips and buses, routing of strips.
    pub fn param_names(&self) -> Vec<String> {
        let channels = self.strip_names().into_iter().chain(self.bus_names());
        let mut names: Vec<String> = channels
            .flat_map(|c| [param_name(&c, "Gain"), param_name(&c, "Mute")])
            .collect();
        for strip in self.strip_names() {
            names.extend(
                self.bus_labels()
                    .into_iter()
                    .map(|label| param_name(&strip, &label)),
            );
        }
        names
    }
}

/// Parameter name such as `Strip[2].A1` or `Bus[0].Gain`.
pub fn param_name(target: &str, field: &str) -> String {
    format!("{target}.{field}")
}

/// Dropdown entries for the settings source `source` of an action.
pub fn dropdown(layout: &Layout, source: &str) -> Option<Vec<(String, String)>> {
    let names: Vec<String> = match source {
        "targets" => layout
            .strip_names()
            .into_iter()
            .chain(layout.bus_names())
            .collect(),
        "strips" => layout.strip_names(),
        "buses" => layout.bus_labels(),
        _ => return None,
    };
    Some(names.into_iter().map(|name| (name.clone(), name)).collect())
}

/// Gain in dB, kept in the range VoiceMeeter accepts.
pub fn clamp_db(db: f32) -> f32 {
    db.clamp(MIN_DB, MAX_DB)
}

/// Gain as shown on a key, e.g. `-6,0 dB`; values that round to zero show `0,0 dB`.
pub fn db_title(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{db:.1} dB").replace('.', ",")
}

/// Parameter values of one connection, read after VoiceMeeter reported a change.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub layout: Layout,
    pub values: HashMap<String, f32>,
}

/// Messages from the DLL thread to the main loop.
pub enum Event {
    /// No connection; the text is the reason shown in the status.
    Offline(String),
    Snapshot(Snapshot),
}

/// Requests for the DLL thread.
pub enum Command {
    Set(String, f32),
    Script(String),
}

struct Plugin {
    host: Host,
    contexts: Contexts,
    global: Map<String, Value>,
    snapshot: Option<Snapshot>,
}

impl Plugin {
    fn new(host: Host) -> Self {
        Self {
            host,
            contexts: Contexts::default(),
            global: Map::new(),
            snapshot: None,
        }
    }

    fn value(&self, param: &str) -> Option<f32> {
        self.snapshot.as_ref()?.values.get(param).copied()
    }

    fn flag(&self, param: &str) -> bool {
        self.value(param).is_some_and(|value| value > 0.5)
    }

    fn set_status(&mut self, status: String) {
        if self.global.get("status").and_then(Value::as_str) != Some(status.as_str()) {
            self.global.insert("status".into(), json!(status));
            self.host.save_global(&self.global);
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Offline(reason) => {
                self.snapshot = None;
                self.set_status(reason);
            }
            Event::Snapshot(snapshot) => {
                self.set_status(format!("Verbunden mit {}", snapshot.layout.name));
                self.snapshot = Some(snapshot);
            }
        }
        self.refresh_all();
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        match inst.kind() {
            "gain" => {
                let target = inst.text("target");
                let muted = self.flag(&param_name(&target, "Mute"));
                let title = self
                    .value(&param_name(&target, "Gain"))
                    .map_or(ERROR_TITLE.to_owned(), db_title);
                (u16::from(muted), title)
            }
            "mute" => {
                let muted = self.flag(&param_name(&inst.text("target"), "Mute"));
                (u16::from(muted), String::new())
            }
            "route" => {
                let on = self.flag(&param_name(&inst.text("strip"), &inst.text("bus")));
                (u16::from(on), String::new())
            }
            _ => (0, String::new()),
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

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        match inst.kind() {
            "gain" | "mute" => self.toggle(context, &param_name(&inst.text("target"), "Mute")),
            "route" => self.toggle(context, &param_name(&inst.text("strip"), &inst.text("bus"))),
            "script" => self.run(context, Command::Script(inst.text("script"))),
            _ => {}
        }
    }

    /// Flips a 0/1 parameter; without a known value there is nothing to flip.
    fn toggle(&self, context: &str, param: &str) {
        match self.value(param) {
            Some(_) => {
                let next = if self.flag(param) { 0.0 } else { 1.0 };
                self.run(context, Command::Set(param.to_owned(), next));
            }
            None => self.host.alert(context),
        }
    }

    fn rotate(&self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        if inst.kind() != "gain" || ticks == 0 {
            return;
        }
        let param = param_name(&inst.text("target"), "Gain");
        let step = inst.number("step", DEFAULT_STEP) as f32;
        match self.value(&param) {
            Some(db) => {
                let next = clamp_db(db + ticks as f32 * step);
                self.run(context, Command::Set(param, next));
            }
            None => self.host.alert(context),
        }
    }

    /// Runs in its own task so a slow reply from the DLL thread does not block the main loop.
    fn run(&self, context: &str, command: Command) {
        let (host, context) = (self.host.clone(), context.to_owned());
        tokio::spawn(async move {
            if let Err(err) = platform::run(command).await {
                host.alert(&context);
                host.log(format!("VoiceMeeter-Befehl fehlgeschlagen: {err:#}"));
            }
        });
    }

    fn on_request(&self, req: Value) {
        let source = req["request"].as_str().unwrap_or_default();
        let options = match self.snapshot.as_ref() {
            Some(snapshot) => dropdown(&snapshot.layout, source).context("unbekannte Anfrage"),
            None => Err(anyhow!(NOT_CONNECTED)),
        };
        self.host.reply_options(&req, options);
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.refresh(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => {
                self.global = msg["payload"]["settings"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
            }
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => self.rotate(&context, msg["payload"]["ticks"].as_i64().unwrap_or(0)),
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    platform::start(tx);
    let mut plugin = Plugin::new(host);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg),
                None => break,
            },
            Some(event) = events.recv() => plugin.on_event(event),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_follows_the_edition() {
        let standard = Layout::from_type(1).unwrap();
        assert_eq!(
            (standard.strips, standard.buses_a, standard.buses_b),
            (3, 1, 1)
        );
        assert_eq!(standard.bus_labels(), ["A1", "B1"]);
        let banana = Layout::from_type(2).unwrap();
        assert_eq!(banana.bus_labels(), ["A1", "A2", "A3", "B1", "B2"]);
        assert_eq!(Layout::from_type(6), Layout::from_type(3));
        assert_eq!(Layout::from_type(3).unwrap().strips, 8);
        assert_eq!(Layout::from_type(4), None);
    }

    #[test]
    fn parameter_names_match_the_remote_api() {
        assert_eq!(param_name("Strip[2]", "A1"), "Strip[2].A1");
        assert_eq!(param_name("Bus[7]", "Mute"), "Bus[7].Mute");
        let potato = Layout::from_type(3).unwrap().param_names();
        assert!(potato.contains(&"Strip[7].B3".to_owned()));
        assert!(potato.contains(&"Bus[7].Gain".to_owned()));
        // Gain and mute for 8 strips and 8 buses, plus one route per strip and bus.
        assert_eq!(potato.len(), 16 * 2 + 8 * 8);
    }

    #[test]
    fn dropdowns_list_the_edition_channels() {
        let standard = Layout::from_type(1).unwrap();
        let targets: Vec<String> = dropdown(&standard, "targets")
            .unwrap()
            .into_iter()
            .map(|(value, _)| value)
            .collect();
        assert_eq!(
            targets,
            ["Strip[0]", "Strip[1]", "Strip[2]", "Bus[0]", "Bus[1]"]
        );
        assert_eq!(dropdown(&standard, "buses").unwrap().len(), 2);
        assert!(dropdown(&standard, "unknown").is_none());
    }

    #[test]
    fn gain_is_clamped_and_formatted_in_german() {
        assert_eq!(clamp_db(-80.0), -60.0);
        assert_eq!(clamp_db(20.0), 12.0);
        assert_eq!(db_title(-6.0), "-6,0 dB");
        assert_eq!(db_title(12.0), "12,0 dB");
        assert_eq!(db_title(-0.01), "0,0 dB");
    }

    #[test]
    fn keys_show_the_snapshot_and_the_offline_state() {
        let (host, mut rx) = Host::channel("de.opendeckn3.voicemeeter");
        let mut plugin = Plugin::new(host);
        plugin.on_host(json!({ "event": "willAppear", "context": "g",
            "action": "de.opendeckn3.voicemeeter.gain",
            "payload": { "controller": "Keypad", "settings": { "target": "Bus[0]" } } }));
        let snapshot = Snapshot {
            layout: Layout::from_type(1).unwrap(),
            values: HashMap::from([
                ("Bus[0].Gain".to_owned(), -6.0),
                ("Bus[0].Mute".to_owned(), 1.0),
            ]),
        };
        plugin.on_event(Event::Snapshot(snapshot));
        let sent: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|m| m["event"] == "setState" && m["payload"]["state"] == 1)
        );
        assert!(
            sent.iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == "-6,0 dB")
        );

        plugin.on_event(Event::Offline("VoiceMeeter läuft nicht".into()));
        let sent: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|m| m["event"] == "setState" && m["payload"]["state"] == 0)
        );
        assert!(
            sent.iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == "–")
        );
        assert!(sent.iter().any(|m| m["event"] == "setGlobalSettings"
            && m["payload"]["status"] == "VoiceMeeter läuft nicht"));
    }
}

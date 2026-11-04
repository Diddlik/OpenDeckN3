#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 AudioRouter plugin: play/stop, sessions, privacy mute, tool
//! toggles, levels, recording and soundboard of AudioRouter
//! (https://audiorouter.org) through its local HTTP API
//! (see `docs/PLUGIN_AUDIOROUTER.md`).

use std::{
    collections::HashMap,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail};
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const DEFAULT_URL: &str = "http://127.0.0.1:17891";
const OFFLINE: &str = "offline";
const UNSET: &str = "–";
const NOT_CONNECTED: &str = "AudioRouter ist nicht verbunden";
const POLL_EVERY: Duration = Duration::from_secs(1);

/// Client for AudioRouter's local HTTP API: every backend method `a.b` is `POST /api/v1/a/b`.
#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Api {
    fn new(base: &str, token: &str) -> anyhow::Result<Self> {
        let base = Some(base.trim())
            .filter(|b| !b.is_empty())
            .unwrap_or(DEFAULT_URL)
            .trim_end_matches('/');
        anyhow::ensure!(
            base.starts_with("http://") || base.starts_with("https://"),
            "Adresse der AudioRouter-API eintragen, z. B. {DEFAULT_URL}"
        );
        anyhow::ensure!(
            !token.trim().is_empty(),
            "API-Token fehlt: in AudioRouter unter API → „Copy API token“ kopieren"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!(
                    "OpenDeckN3-AudioRouter/",
                    env!("CARGO_PKG_VERSION")
                ))
                .timeout(Duration::from_secs(3))
                .build()?,
            base: base.to_owned(),
            token: token.trim().to_owned(),
        })
    }

    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let res = self
            .http
            .post(format!("{}{}", self.base, method_path(method)))
            .bearer_auth(&self.token)
            .json(&params)
            .send()
            .await
            .map_err(|_| {
                anyhow!(
                    "AudioRouter nicht erreichbar ({}). Läuft die API (AudioRouter → API → Start API)?",
                    self.base
                )
            })?;
        let status = res.status();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(body);
        }
        if let Some(message) = body["error"]["message"].as_str() {
            bail!("{message}");
        }
        match status.as_u16() {
            401 => bail!("API-Token ungültig: in AudioRouter unter API neu kopieren"),
            403 => bail!("Zugriff verweigert: Adresse mit 127.0.0.1 statt localhost eintragen"),
            code => bail!("AudioRouter antwortet mit HTTP {code}"),
        }
    }
}

fn method_path(method: &str) -> String {
    format!("/api/v1/{}", method.replace('.', "/"))
}

/// Every press is a new operation, so it gets a fresh key.
fn idempotency_key(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("opendeckn3-{prefix}-{ms}-{n}")
}

/// What one poll reads; `None` fields were not requested.
struct Polled {
    catalog: Option<Value>,
    summary: Value,
    parameters: Option<HashMap<String, Value>>,
    sessions: Option<Vec<(String, String)>>,
    recorders: Option<HashMap<String, String>>,
}

struct PollPlan {
    catalog: bool,
    sessions: bool,
    recorders: bool,
    /// Session id and revision of the parameters already read.
    known: Option<(String, Value)>,
}

async fn poll(api: Api, plan: PollPlan) -> anyhow::Result<Polled> {
    let catalog = match plan.catalog {
        true => Some(api.call("nodes.catalog", json!({})).await?),
        false => None,
    };
    let summary = api.call("sessions.summary", json!({})).await?;
    let session = summary["sessionId"].as_str().unwrap_or_default().to_owned();
    // The summary has no tool settings; the full session is read only when its revision changes.
    let parameters = if plan.known != Some((session.clone(), summary["revision"].clone())) {
        let raw = api
            .call("sessions.get", json!({ "sessionId": session }))
            .await?;
        Some(node_parameters(&raw))
    } else {
        None
    };
    let sessions = match plan.sessions {
        true => Some(session_items(
            &api.call("sessions.list", json!({ "limit": 100 })).await?,
        )),
        false => None,
    };
    let recorders = match plan.recorders {
        true => Some(recorder_states(
            &api.call("recorders.list", json!({})).await?,
            &session,
        )),
        false => None,
    };
    Ok(Polled {
        catalog,
        summary,
        parameters,
        sessions,
        recorders,
    })
}

fn node_parameters(session: &Value) -> HashMap<String, Value> {
    nodes(session)
        .filter_map(|n| Some((n["id"].as_str()?.to_owned(), n["parameters"].clone())))
        .collect()
}

fn session_items(list: &Value) -> Vec<(String, String)> {
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let id = s["id"].as_str()?;
            Some((id.to_owned(), s["name"].as_str().unwrap_or(id).to_owned()))
        })
        .collect()
}

/// Recorder node → state (`recording`, `paused`, `idle` …) in one session.
fn recorder_states(list: &Value, session: &str) -> HashMap<String, String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["sessionId"] == session)
        .filter_map(|r| {
            Some((
                r["nodeId"].as_str()?.to_owned(),
                r["state"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn nodes(value: &Value) -> impl Iterator<Item = &Value> {
    value["nodes"].as_array().into_iter().flatten()
}

/// A catalog parameter spec of a node kind.
fn spec<'a>(catalog: &'a Value, kind: &str, name: &str) -> Option<&'a Value> {
    params_of(catalog, kind).find(|p| p["name"] == name)
}

fn params_of<'a>(catalog: &'a Value, kind: &str) -> impl Iterator<Item = &'a Value> {
    catalog
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |k| k["kind"] == kind)
        .flat_map(|k| k["parameters"].as_array().into_iter().flatten())
}

fn two_choices(spec: Option<&Value>) -> Option<&Vec<Value>> {
    spec.and_then(|s| s["enum"].as_array())
        .filter(|choices| choices.len() == 2)
}

/// Booleans are on when true; two-choice settings when the second choice is selected.
fn is_on(value: &Value, spec: Option<&Value>) -> bool {
    match two_choices(spec) {
        Some(choices) => *value == choices[1],
        None => *value == Value::Bool(true),
    }
}

fn opposite(value: &Value, spec: Option<&Value>) -> Value {
    match two_choices(spec) {
        Some(choices) if *value == choices[0] => choices[1].clone(),
        Some(choices) => choices[0].clone(),
        None => Value::Bool(*value != Value::Bool(true)),
    }
}

/// Settings a toggle key can flip: Enabled, Bypass and on/off or two-choice settings.
fn toggle_targets(catalog: &Value, kind: &str) -> Vec<(String, String)> {
    let mut targets = vec![
        ("enabled".to_owned(), "Aktiv (Enabled)".to_owned()),
        ("bypass".to_owned(), "Umgangen (Bypass)".to_owned()),
    ];
    targets.extend(
        params_of(catalog, kind)
            .filter(|p| p["type"] == "boolean" || two_choices(Some(p)).is_some())
            .filter_map(|p| p["name"].as_str())
            .map(|name| (name.to_owned(), name.to_owned())),
    );
    targets
}

fn number_params(catalog: &Value, kind: &str) -> Vec<(String, String)> {
    params_of(catalog, kind)
        .filter(|p| p["type"] == "number")
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let label = match p["unit"].as_str() {
                Some(unit) => format!("{name} ({unit})"),
                None => name.to_owned(),
            };
            Some((name.to_owned(), label))
        })
        .collect()
}

/// Next value of a numeric setting, kept in its range and rounded to the step's precision
/// so repeated presses do not drift.
fn step_value(current: f64, step: f64, spec: Option<&Value>) -> f64 {
    let min = spec.and_then(|s| s["minimum"].as_f64()).unwrap_or(f64::MIN);
    let max = spec.and_then(|s| s["maximum"].as_f64()).unwrap_or(f64::MAX);
    let scale = 1000.0;
    ((current + step).clamp(min, max) * scale).round() / scale
}

/// Value as shown on a key, e.g. `60%` or `-6,5 dB`.
fn format_value(value: f64, unit: &str) -> String {
    let text = if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}").replace('.', ",")
    };
    match unit {
        "" => text,
        "%" => format!("{text}%"),
        unit => format!("{text} {unit}"),
    }
}

fn format_elapsed(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match h {
        0 => format!("{m:02}:{s:02}"),
        h => format!("{h}:{m:02}:{s:02}"),
    }
}

/// Selecting alone never stops audio, and only one session can be prepared at a time
/// (Play on another session is refused while one plays). So: stop what plays, select,
/// and play the new session if audio was playing before or the key asks for it.
async fn switch_session(
    api: Api,
    to: String,
    current: String,
    play_after: bool,
) -> anyhow::Result<()> {
    let status = api.call("status.get", json!({})).await?;
    let playing: Vec<&str> = status["activeSessionIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if to == current && playing.contains(&to.as_str()) {
        return Ok(());
    }
    for id in playing.iter().filter(|id| **id != to) {
        api.call(
            "session.stop",
            json!({ "sessionId": id, "idempotencyKey": idempotency_key("switch-stop") }),
        )
        .await?;
    }
    if to != current {
        api.call(
            "sessions.active.set",
            json!({ "sessionId": to, "idempotencyKey": idempotency_key("session") }),
        )
        .await?;
    }
    if (play_after || !playing.is_empty()) && !playing.contains(&to.as_str()) {
        api.call(
            "sessions.play",
            json!({ "sessionId": to, "idempotencyKey": idempotency_key("play") }),
        )
        .await?;
    }
    Ok(())
}

/// `nodes.set` body fields that set `target` to `value`.
fn set_change(target: &str, value: Value) -> Value {
    match target {
        "enabled" | "bypass" => json!({ target: value }),
        _ => json!({ "parameters": { target: value } }),
    }
}

enum Msg {
    Host(Value),
    Polled(u64, anyhow::Result<Polled>),
    Done(u64),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    api: Option<Api>,
    generation: u64,
    polling: bool,
    online: bool,
    catalog: Value,
    summary: Value,
    parameters: HashMap<String, Value>,
    known: Option<(String, Value)>,
    sessions: Vec<(String, String)>,
    recorders: HashMap<String, String>,
    recording_since: HashMap<String, Instant>,
    /// Momentary toggle keys: the value to restore on release.
    held: HashMap<String, Value>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            global: Map::new(),
            api: None,
            generation: 0,
            polling: false,
            online: false,
            catalog: Value::Null,
            summary: Value::Null,
            parameters: HashMap::new(),
            known: None,
            sessions: Vec::new(),
            recorders: HashMap::new(),
            recording_since: HashMap::new(),
            held: HashMap::new(),
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
        let changed = |k: &str| previous.get(k) != self.global.get(k);
        if !(self.api.is_none() || ["url", "token"].into_iter().any(changed)) {
            return;
        }
        self.generation += 1;
        self.polling = false;
        self.go_offline();
        self.api = match Api::new(
            &global_str(&self.global, "url"),
            &global_str(&self.global, "token"),
        ) {
            Ok(api) => Some(api),
            Err(err) => {
                self.set_status(format!("{err:#}"));
                None
            }
        };
        if self.api.is_some() {
            self.set_status("Verbinde mit AudioRouter …");
            self.poll();
        }
        self.refresh_all();
    }

    fn go_offline(&mut self) {
        self.online = false;
        self.catalog = Value::Null;
        self.summary = Value::Null;
        self.parameters.clear();
        self.known = None;
        self.sessions.clear();
        self.recorders.clear();
        self.recording_since.clear();
    }

    fn poll(&mut self) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if self.polling {
            return;
        }
        self.polling = true;
        let plan = PollPlan {
            catalog: self.catalog.is_null(),
            sessions: self.contexts.any("session"),
            recorders: self.contexts.any("record"),
            known: self.known.clone(),
        };
        let (tx, generation) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            tx.send(Msg::Polled(generation, poll(api, plan).await)).ok();
        });
    }

    fn on_polled(&mut self, generation: u64, result: anyhow::Result<Polled>) {
        if generation != self.generation {
            return;
        }
        self.polling = false;
        match result {
            Ok(p) => {
                if let Some(catalog) = p.catalog {
                    self.catalog = catalog;
                }
                if let Some(parameters) = p.parameters {
                    self.parameters = parameters;
                    self.known = Some((
                        p.summary["sessionId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        p.summary["revision"].clone(),
                    ));
                }
                if let Some(sessions) = p.sessions {
                    self.sessions = sessions;
                }
                if let Some(recorders) = p.recorders {
                    self.recorders = recorders;
                }
                self.summary = p.summary;
                self.online = true;
                self.track_recordings();
                let name = self.summary["name"].as_str().unwrap_or_default();
                self.set_status(format!("Verbunden, Session „{name}“"));
            }
            Err(err) => {
                self.go_offline();
                self.set_status(format!("Nicht verbunden: {err:#}"));
            }
        }
        self.refresh_all();
    }

    fn is_recording(&self, node: &str) -> bool {
        matches!(
            self.recorders.get(node).map(String::as_str),
            Some("recording" | "paused")
        )
    }

    /// The API reports no start time, so elapsed time counts from when a recording was first seen.
    fn track_recordings(&mut self) {
        let recording: Vec<String> = self
            .recorders
            .keys()
            .filter(|n| self.is_recording(n))
            .cloned()
            .collect();
        self.recording_since.retain(|n, _| recording.contains(n));
        for node in recording {
            self.recording_since
                .entry(node)
                .or_insert_with(Instant::now);
        }
    }

    fn playing(&self) -> bool {
        self.summary["playing"].as_bool().unwrap_or(false)
    }

    fn session_id(&self) -> String {
        self.summary["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn node(&self, id: &str) -> Option<&Value> {
        nodes(&self.summary).find(|n| n["id"] == id && !id.is_empty())
    }

    fn node_spec(&self, node: &Value, name: &str) -> Option<&Value> {
        spec(
            &self.catalog,
            node["kind"].as_str().unwrap_or_default(),
            name,
        )
    }

    /// A node's current value of a setting: saved, else the catalog default.
    fn setting(&self, node: &Value, name: &str) -> Value {
        if matches!(name, "enabled" | "bypass") {
            return node[name].clone();
        }
        let id = node["id"].as_str().unwrap_or_default();
        match self.parameters.get(id).map(|p| &p[name]) {
            Some(v) if !v.is_null() => v.clone(),
            _ => self
                .node_spec(node, name)
                .map_or(Value::Null, |s| s["default"].clone()),
        }
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        if !self.online {
            return (0, OFFLINE.into());
        }
        match inst.kind() {
            "play" => (u16::from(self.playing()), String::new()),
            "privacy" => {
                let muted = self.summary["privacyMuted"].as_bool().unwrap_or(false);
                (u16::from(muted), String::new())
            }
            "toggle" => match self.node(&inst.text("node")) {
                Some(node) => {
                    let target = inst.text_or("target", "enabled");
                    let on = is_on(&self.setting(node, &target), self.node_spec(node, &target));
                    (u16::from(on), String::new())
                }
                None => (0, UNSET.into()),
            },
            "level" => {
                let param = inst.text("parameter");
                let title = self
                    .node(&inst.text("node"))
                    .and_then(|node| {
                        let value = self.setting(node, &param).as_f64()?;
                        let unit = self
                            .node_spec(node, &param)
                            .and_then(|s| s["unit"].as_str())
                            .unwrap_or_default();
                        Some(format_value(value, unit))
                    })
                    .unwrap_or_else(|| UNSET.into());
                (0, title)
            }
            "session" => {
                let id = inst.text("session");
                let name = self
                    .sessions
                    .iter()
                    .find(|(sid, _)| *sid == id)
                    .map_or(id.clone(), |(_, name)| name.clone());
                (u16::from(!id.is_empty() && id == self.session_id()), name)
            }
            "record" => {
                let node = inst.text("recorder");
                match self.recording_since.get(&node) {
                    Some(since) => (1, format_elapsed(since.elapsed().as_secs())),
                    None => (0, String::new()),
                }
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

    /// Runs one operation in its own task and reads the new state right after it.
    fn run<F>(&self, context: &str, job: impl FnOnce(Api) -> F)
    where
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let Some(api) = self.api.clone().filter(|_| self.online) else {
            self.host.alert(context);
            return;
        };
        let future = job(api);
        let (host, tx, context, generation) = (
            self.host.clone(),
            self.tx.clone(),
            context.to_owned(),
            self.generation,
        );
        tokio::spawn(async move {
            if let Err(err) = future.await {
                host.alert(&context);
                host.log(format!("AudioRouter: {err:#}"));
            }
            tx.send(Msg::Done(generation)).ok();
        });
    }

    fn call(&self, context: &str, method: &'static str, params: Value) {
        self.run(context, move |api| async move {
            api.call(method, params).await.map(drop)
        });
    }

    fn key_down(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        let session = self.session_id();
        match inst.kind() {
            "play" => {
                let method = if self.playing() {
                    "session.stop"
                } else {
                    "sessions.play"
                };
                let params =
                    json!({ "sessionId": session, "idempotencyKey": idempotency_key("play") });
                self.call(context, method, params);
            }
            "privacy" => self.call(
                context,
                "safety.togglePrivacyMute",
                json!({ "idempotencyKey": idempotency_key("privacy") }),
            ),
            "toggle" => {
                let Some(node) = self.node(&inst.text("node")).cloned() else {
                    return self.host.alert(context);
                };
                let target = inst.text_or("target", "enabled");
                if inst.text("mode") == "momentary" {
                    // Explicit values: a lost release must not leave the setting flipped twice.
                    let original = self.setting(&node, &target);
                    let next = opposite(&original, self.node_spec(&node, &target));
                    self.held.insert(context.to_owned(), original);
                    self.set_node(context, &node, &target, next);
                } else {
                    self.call(
                        context,
                        "nodes.toggle",
                        json!({ "node": node["id"], "target": target, "idempotencyKey": idempotency_key("toggle") }),
                    );
                }
            }
            "level" => self.step(context, 1),
            "session" => {
                let to = inst.text("session");
                if to.is_empty() {
                    return self.host.alert(context);
                }
                let play_after = inst.flag("play", false);
                self.run(context, move |api| {
                    switch_session(api, to, session, play_after)
                });
            }
            "record" => {
                let node = inst.text("recorder");
                let recording = self.is_recording(&node);
                if self.node(&node).is_none() || (!recording && !self.playing()) {
                    // Like AudioRouter's Record button: a Recorder only saves what plays through it.
                    return self.host.alert(context);
                }
                let method = if recording {
                    "recorders.stopRecording"
                } else {
                    "recorders.startRecording"
                };
                self.call(
                    context,
                    method,
                    json!({ "sessionId": session, "nodeId": node, "idempotencyKey": idempotency_key("record") }),
                );
            }
            "sound" => self.sound(context, &inst, true),
            _ => {}
        }
    }

    fn key_up(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        match inst.kind() {
            "toggle" => {
                if let Some(original) = self.held.remove(context)
                    && let Some(node) = self.node(&inst.text("node")).cloned()
                {
                    self.set_node(context, &node, &inst.text_or("target", "enabled"), original);
                }
            }
            "sound" => self.sound(context, &inst, false),
            _ => {}
        }
    }

    fn set_node(&self, context: &str, node: &Value, target: &str, value: Value) {
        let mut params = set_change(target, value);
        params["node"] = node["id"].clone();
        params["idempotencyKey"] = json!(idempotency_key("set"));
        self.call(context, "nodes.set", params);
    }

    fn step(&self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let param = inst.text("parameter");
        let Some(node) = self.node(&inst.text("node")) else {
            return self.host.alert(context);
        };
        let Some(current) = self.setting(node, &param).as_f64() else {
            return self.host.alert(context);
        };
        let step = inst.number("step", 1.0) * ticks as f64;
        let next = step_value(current, step, self.node_spec(node, &param));
        self.set_node(context, node, &param, json!(next));
    }

    /// A press plays the slot; a release only matters for sounds that play while held.
    fn sound(&self, context: &str, inst: &Instance, press: bool) {
        let node_id = inst.text("soundboard");
        let slot = inst.number("slot", 1.0) as u64;
        let behavior = inst.text_or("behavior", "slot");
        let Some(node) = self.node(&node_id) else {
            if press {
                self.host.alert(context);
            }
            return;
        };
        if !press {
            let mode = match behavior.as_str() {
                "slot" => self.setting(node, &format!("slot{slot}Mode")),
                other => json!(other),
            };
            if mode != "hold" {
                return;
            }
        }
        let mut params = json!({
            "sessionId": self.session_id(),
            "nodeId": node_id,
            "slot": slot,
            "action": if press { "press" } else { "release" },
        });
        if behavior != "slot" {
            params["mode"] = json!(behavior);
        }
        self.call(context, "soundboard.trigger", params);
    }

    fn node_options(&self, keep: impl Fn(&Value) -> bool) -> Vec<(String, String)> {
        nodes(&self.summary)
            .filter(|n| keep(n))
            .filter_map(|n| {
                let id = n["id"].as_str()?;
                let name = n["name"].as_str().unwrap_or(id);
                let kind = n["kind"].as_str().unwrap_or_default();
                Some((id.to_owned(), format!("{name} · {kind}")))
            })
            .collect()
    }

    fn on_request(&self, req: Value) {
        let source = req["request"].as_str().unwrap_or_default();
        if !self.online {
            return self.host.reply_options(&req, Err(anyhow!(NOT_CONNECTED)));
        }
        let kind_of = |key: &str| {
            self.node(req[key].as_str().unwrap_or_default())
                .and_then(|n| n["kind"].as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let options = match source {
            "nodes" => self.node_options(|_| true),
            "levelNodes" => self.node_options(|n| {
                !number_params(&self.catalog, n["kind"].as_str().unwrap_or_default()).is_empty()
            }),
            "recorders" => self.node_options(|n| n["kind"] == "recorder"),
            "soundboards" => self.node_options(|n| n["kind"] == "soundboard"),
            "targets" => toggle_targets(&self.catalog, &kind_of("node")),
            "numbers" => number_params(&self.catalog, &kind_of("node")),
            // The session list is read only while a session key is visible, so ask now.
            "sessions" => {
                let (api, host) = (self.api.clone(), self.host.clone());
                tokio::spawn(async move {
                    let result = match api {
                        Some(api) => api
                            .call("sessions.list", json!({ "limit": 100 }))
                            .await
                            .map(|list| session_items(&list)),
                        None => Err(anyhow!(NOT_CONNECTED)),
                    };
                    host.reply_options(&req, result);
                });
                return;
            }
            _ => {
                return self
                    .host
                    .reply_options(&req, Err(anyhow!("unbekannte Anfrage")));
            }
        };
        self.host.reply_options(&req, Ok(options));
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            if msg["event"] == "willAppear" {
                // A newly visible session or record key may need lists the last poll skipped.
                self.poll();
            }
            self.refresh(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            "keyDown" => self.key_down(&context),
            "keyUp" => self.key_up(&context),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                if ticks != 0 {
                    self.step(&context, ticks);
                }
            }
            "willDisappear" => {
                self.held.remove(&context);
            }
            _ => {}
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Polled(generation, result) => self.on_polled(generation, result),
            Msg::Done(generation) if generation == self.generation => self.poll(),
            Msg::Done(_) => {}
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut plugin = Plugin::new(host, tx.clone());
    let mut tick = tokio::time::interval(POLL_EVERY);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_msg(Msg::Host(msg)),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.on_msg(msg),
            _ = tick.tick() => {
                plugin.poll();
                // Recording keys show the elapsed time.
                plugin.refresh_all();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Value {
        json!([{ "kind": "volume", "name": "Volume", "parameters": [
            { "name": "percent", "type": "number", "minimum": 0, "maximum": 200, "default": 100, "unit": "%" },
            { "name": "polarity", "type": "string", "enum": ["normal", "inverted"], "default": "normal" },
            { "name": "mono", "type": "boolean", "default": false },
            { "name": "curve", "type": "string", "enum": ["a", "b", "c"] }
        ] }])
    }

    fn sent(rx: &mut mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn methods_map_to_http_paths() {
        assert_eq!(method_path("sessions.summary"), "/api/v1/sessions/summary");
        assert_eq!(
            method_path("sessions.active.set"),
            "/api/v1/sessions/active/set"
        );
        assert_ne!(idempotency_key("a"), idempotency_key("a"));
    }

    #[test]
    fn api_needs_http_address_and_token() {
        assert!(Api::new("", "secret").is_ok_and(|a| a.base == DEFAULT_URL));
        assert!(
            Api::new("http://192.168.1.20:17891/", "secret")
                .is_ok_and(|a| a.base == "http://192.168.1.20:17891")
        );
        assert!(Api::new("127.0.0.1:17891", "secret").is_err());
        assert!(Api::new(DEFAULT_URL, " ").is_err());
    }

    #[test]
    fn switches_follow_booleans_and_two_choices() {
        let cat = catalog();
        let polarity = spec(&cat, "volume", "polarity");
        assert!(is_on(&json!("inverted"), polarity));
        assert!(!is_on(&json!("normal"), polarity));
        assert_eq!(opposite(&json!("normal"), polarity), json!("inverted"));
        assert_eq!(opposite(&json!("inverted"), polarity), json!("normal"));
        assert!(is_on(&json!(true), None));
        assert_eq!(opposite(&Value::Null, None), json!(true));
        assert_eq!(opposite(&json!(true), None), json!(false));
    }

    #[test]
    fn dropdowns_come_from_the_catalog() {
        let cat = catalog();
        let targets: Vec<String> = toggle_targets(&cat, "volume")
            .into_iter()
            .map(|(v, _)| v)
            .collect();
        assert_eq!(targets, ["enabled", "bypass", "polarity", "mono"]);
        assert_eq!(
            number_params(&cat, "volume"),
            [("percent".to_owned(), "percent (%)".to_owned())]
        );
        assert!(number_params(&cat, "unknown").is_empty());
    }

    #[test]
    fn steps_stay_in_range_without_drift() {
        let cat = catalog();
        let percent = spec(&cat, "volume", "percent");
        assert_eq!(step_value(195.0, 10.0, percent), 200.0);
        assert_eq!(step_value(5.0, -10.0, percent), 0.0);
        assert_eq!(step_value(0.1, 0.2, None), 0.3);
        assert_eq!(format_value(60.0, "%"), "60%");
        assert_eq!(format_value(-6.5, "dB"), "-6,5 dB");
        assert_eq!(format_value(2.0, ""), "2");
        assert_eq!(format_elapsed(65), "01:05");
        assert_eq!(format_elapsed(3725), "1:02:05");
    }

    #[test]
    fn parses_api_lists() {
        let list = json!({ "items": [{ "id": "a", "name": "Gaming" }, { "id": "b" }] });
        assert_eq!(
            session_items(&list),
            [
                ("a".to_owned(), "Gaming".to_owned()),
                ("b".to_owned(), "b".to_owned())
            ]
        );
        let recorders = json!([
            { "sessionId": "a", "nodeId": "rec", "state": "recording" },
            { "sessionId": "b", "nodeId": "other", "state": "idle" },
            { "sessionId": "a", "nodeId": null, "state": "idle" }
        ]);
        assert_eq!(
            recorder_states(&recorders, "a"),
            HashMap::from([("rec".to_owned(), "recording".to_owned())])
        );
        let session = json!({ "nodes": [{ "id": "vol", "parameters": { "percent": 60 } }] });
        assert_eq!(node_parameters(&session)["vol"]["percent"], 60);
    }

    #[test]
    fn keys_show_state_and_offline() {
        let (host, mut rx) = Host::channel("de.opendeckn3.audiorouter");
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut plugin = Plugin::new(host, tx);
        let appear = |context: &str, kind: &str, settings: Value| {
            json!({ "event": "willAppear", "context": context,
                "action": format!("de.opendeckn3.audiorouter.{kind}"),
                "payload": { "controller": "Keypad", "settings": settings } })
        };
        plugin.on_host(appear("p", "privacy", json!({})));
        plugin.on_host(appear(
            "l",
            "level",
            json!({ "node": "vol", "parameter": "percent" }),
        ));
        plugin.on_host(appear(
            "t",
            "toggle",
            json!({ "node": "vol", "target": "polarity" }),
        ));
        assert!(
            sent(&mut rx)
                .iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == OFFLINE)
        );

        plugin.on_polled(
            0,
            Ok(Polled {
                catalog: Some(catalog()),
                summary: json!({ "sessionId": "s", "name": "Gaming", "revision": 3, "playing": true,
                    "privacyMuted": true,
                    "nodes": [{ "id": "vol", "name": "Game volume", "kind": "volume", "enabled": true, "bypass": false }] }),
                parameters: Some(HashMap::from([(
                    "vol".to_owned(),
                    json!({ "percent": 60, "polarity": "inverted" }),
                )])),
                sessions: None,
                recorders: None,
            }),
        );
        let out = sent(&mut rx);
        let title = |c: &str| {
            out.iter()
                .rev()
                .find(|m| m["event"] == "setTitle" && m["context"] == c)
                .map(|m| m["payload"]["title"].clone())
        };
        let state = |c: &str| {
            out.iter()
                .rev()
                .find(|m| m["event"] == "setState" && m["context"] == c)
                .map(|m| m["payload"]["state"].clone())
        };
        assert_eq!(title("l"), Some(json!("60%")));
        assert_eq!(state("p"), Some(json!(1)));
        assert_eq!(state("t"), Some(json!(1)));
        assert_eq!(plugin.known, Some(("s".to_owned(), json!(3))));
        assert!(out.iter().any(|m| m["event"] == "setGlobalSettings"
            && m["payload"]["status"] == "Verbunden, Session „Gaming“"));

        plugin.on_polled(0, Err(anyhow!("weg")));
        let out = sent(&mut rx);
        assert!(
            out.iter()
                .any(|m| m["context"] == "l" && m["payload"]["title"] == OFFLINE)
        );
        assert!(plugin.known.is_none());
    }
}

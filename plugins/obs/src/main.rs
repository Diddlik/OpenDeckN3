#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 OBS plugin: controls OBS Studio over obs-websocket v5
//! (see `docs/PLUGIN_OBS.md`).

mod obs;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use obs::{Endpoint, Obs, ObsEvent};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

enum Msg {
    Host(Value),
    Obs(u64, ObsEvent),
    Connected(u64, anyhow::Result<Obs>),
    /// Scene item id resolved for (scene, source).
    ItemId(u64, String, String, i64),
    Failed(Option<String>, String),
}

#[derive(Default)]
struct Output {
    active: bool,
    paused: bool,
    /// Duration before the current running span (pauses, reconnects).
    offset: Duration,
    running_since: Option<Instant>,
}

impl Output {
    fn elapsed(&self) -> Duration {
        self.offset + self.running_since.map(|s| s.elapsed()).unwrap_or_default()
    }

    fn set(&mut self, active: bool, paused: bool, duration_ms: Option<u64>) {
        if let Some(ms) = duration_ms {
            self.offset = Duration::from_millis(ms);
            self.running_since = (active && !paused).then(Instant::now);
        } else if !active {
            *self = Self::default();
        } else if paused && !self.paused {
            self.offset = self.elapsed();
            self.running_since = None;
        } else if !paused && (self.paused || !self.active) {
            if !self.active {
                self.offset = Duration::ZERO;
            }
            self.running_since = Some(Instant::now());
        }
        self.active = active;
        self.paused = paused && active;
    }
}

#[derive(Default)]
struct State {
    program_scene: String,
    preview_scene: String,
    record: Output,
    stream: Output,
    replay: bool,
    virtualcam: bool,
    studio: bool,
    transition: String,
    muted: HashMap<String, bool>,
    volume_db: HashMap<String, f64>,
    /// (scene, source) → scene item id; (scene, id) → enabled.
    item_ids: HashMap<(String, String), i64>,
    items: HashMap<(String, i64), bool>,
    filters: HashMap<(String, String), bool>,
    stats: Value,
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    loaded: bool,
    obs: Option<Obs>,
    generation: u64,
    connecting: bool,
    state: State,
    pressed_at: HashMap<String, Instant>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            global: Map::new(),
            loaded: false,
            obs: None,
            generation: 0,
            connecting: false,
            state: State::default(),
            pressed_at: HashMap::new(),
        }
    }

    // ------------------------------------------------------------------
    // Connection
    // ------------------------------------------------------------------

    fn endpoint(&self) -> anyhow::Result<Endpoint> {
        if global_str(&self.global, "mode") == "manual" {
            let host = global_str(&self.global, "host");
            Ok(Endpoint {
                host: if host.is_empty() {
                    "127.0.0.1".into()
                } else {
                    host
                },
                port: global_str(&self.global, "port").parse().unwrap_or(4455),
                password: global_str(&self.global, "password"),
            })
        } else {
            obs::local_endpoint()
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
        let first = !self.loaded;
        self.loaded = true;
        if first
            || ["mode", "host", "port", "password", "connect"]
                .into_iter()
                .any(changed)
        {
            self.disconnect();
            self.start_connect();
            // Explicit (re)connect: show that something happens.
            if self.connecting {
                self.set_status("Verbinde mit OBS …");
            }
        }
    }

    fn disconnect(&mut self) {
        if let Some(obs) = self.obs.take() {
            obs.close();
        }
        self.generation += 1;
        self.connecting = false;
        self.state = State::default();
        self.refresh_all();
    }

    fn start_connect(&mut self) {
        if self.connecting {
            return;
        }
        let endpoint = match self.endpoint() {
            Ok(endpoint) => endpoint,
            Err(err) => {
                self.set_status(format!("{err:#}"));
                return;
            }
        };
        self.generation += 1;
        self.connecting = true;
        let (generation, tx) = (self.generation, self.tx.clone());
        tokio::spawn(async move {
            let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
            let forward = tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = ev_rx.recv().await {
                    if forward.send(Msg::Obs(generation, ev)).is_err() {
                        break;
                    }
                }
            });
            let result = Obs::connect(&endpoint, ev_tx).await;
            tx.send(Msg::Connected(generation, result)).ok();
        });
    }

    fn on_connected(&mut self, generation: u64, result: anyhow::Result<Obs>) {
        if generation != self.generation {
            if let Ok(obs) = result {
                obs.close();
            }
            return;
        }
        self.connecting = false;
        let obs = match result {
            Ok(obs) => obs,
            Err(err) => {
                self.set_status(format!("Nicht verbunden: {err:#}"));
                return;
            }
        };
        self.obs = Some(obs.clone());
        self.set_status("Verbunden mit OBS");
        self.host.log("connected to OBS");
        // Initial state, reported as if OBS had sent the events.
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let send = |kind: &str, data: Value| {
                tx.send(Msg::Obs(generation, ObsEvent::Event(kind.into(), data)))
                    .ok();
            };
            if let Ok(v) = obs.call("GetCurrentProgramScene", Value::Null).await {
                send(
                    "CurrentProgramSceneChanged",
                    json!({ "sceneName": v["currentProgramSceneName"] }),
                );
            }
            if let Ok(v) = obs.call("GetRecordStatus", Value::Null).await {
                send("_RecordStatus", v);
            }
            if let Ok(v) = obs.call("GetStreamStatus", Value::Null).await {
                send("_StreamStatus", v);
            }
            if let Ok(v) = obs.call("GetStudioModeEnabled", Value::Null).await {
                send("StudioModeStateChanged", v.clone());
                if v["studioModeEnabled"] == true
                    && let Ok(p) = obs.call("GetCurrentPreviewScene", Value::Null).await
                {
                    send(
                        "CurrentPreviewSceneChanged",
                        json!({ "sceneName": p["currentPreviewSceneName"] }),
                    );
                }
            }
            if let Ok(v) = obs.call("GetCurrentSceneTransition", Value::Null).await {
                send("CurrentSceneTransitionChanged", v);
            }
            if let Ok(v) = obs.call("GetReplayBufferStatus", Value::Null).await {
                send("ReplayBufferStateChanged", v);
            }
            if let Ok(v) = obs.call("GetVirtualCamStatus", Value::Null).await {
                send("VirtualcamStateChanged", v);
            }
        });
        for context in self.contexts.ids() {
            self.fetch_for(&context);
        }
    }

    /// State that depends on a key's settings (sources, filters, inputs).
    fn fetch_for(&self, context: &str) {
        let (Some(obs), Some(inst)) = (self.obs.clone(), self.contexts.get(context)) else {
            return;
        };
        let (tx, generation) = (self.tx.clone(), self.generation);
        let inst = inst.clone();
        tokio::spawn(async move {
            let send = |kind: &str, data: Value| {
                tx.send(Msg::Obs(generation, ObsEvent::Event(kind.into(), data)))
                    .ok();
            };
            match inst.kind() {
                "sourcevisibility" => {
                    let (scene, source) = (inst.text("scene"), inst.text("source"));
                    if scene.is_empty() || source.is_empty() {
                        return;
                    }
                    if let Ok(v) = obs
                        .call(
                            "GetSceneItemId",
                            json!({ "sceneName": scene, "sourceName": source }),
                        )
                        .await
                    {
                        let id = v["sceneItemId"].as_i64().unwrap_or_default();
                        tx.send(Msg::ItemId(generation, scene.clone(), source, id))
                            .ok();
                        if let Ok(e) = obs
                            .call(
                                "GetSceneItemEnabled",
                                json!({ "sceneName": scene, "sceneItemId": id }),
                            )
                            .await
                        {
                            send(
                                "SceneItemEnableStateChanged",
                                json!({ "sceneName": scene, "sceneItemId": id, "sceneItemEnabled": e["sceneItemEnabled"] }),
                            );
                        }
                    }
                }
                "filter" => {
                    let (source, filter) = (inst.text("source"), inst.text("filter"));
                    if let Ok(v) = obs
                        .call(
                            "GetSourceFilter",
                            json!({ "sourceName": source, "filterName": filter }),
                        )
                        .await
                    {
                        send(
                            "SourceFilterEnableStateChanged",
                            json!({ "sourceName": source, "filterName": filter, "filterEnabled": v["filterEnabled"] }),
                        );
                    }
                }
                "audio" => {
                    let input = inst.text("input");
                    if input.is_empty() {
                        return;
                    }
                    if let Ok(v) = obs
                        .call("GetInputMute", json!({ "inputName": input }))
                        .await
                    {
                        send(
                            "InputMuteStateChanged",
                            json!({ "inputName": input, "inputMuted": v["inputMuted"] }),
                        );
                    }
                    if let Ok(v) = obs
                        .call("GetInputVolume", json!({ "inputName": input }))
                        .await
                    {
                        send(
                            "InputVolumeChanged",
                            json!({ "inputName": input, "inputVolumeDb": v["inputVolumeDb"] }),
                        );
                    }
                }
                _ => {}
            }
        });
    }

    fn on_obs(&mut self, generation: u64, event: ObsEvent) {
        if generation != self.generation {
            return;
        }
        let (kind, d) = match event {
            ObsEvent::Closed(reason) => {
                if self.obs.take().is_some() {
                    self.host.log(format!("OBS connection closed: {reason}"));
                }
                self.state = State::default();
                self.set_status("OBS nicht erreichbar – verbinde erneut …");
                self.refresh_all();
                return;
            }
            ObsEvent::Event(kind, data) => (kind, data),
        };
        let s = &mut self.state;
        let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        let output_state = d["outputState"].as_str().unwrap_or_default();
        match kind.as_str() {
            "CurrentProgramSceneChanged" => s.program_scene = text(&d["sceneName"]),
            "CurrentPreviewSceneChanged" => s.preview_scene = text(&d["sceneName"]),
            "_RecordStatus" => s.record.set(
                d["outputActive"] == true,
                d["outputPaused"] == true,
                d["outputDuration"].as_u64(),
            ),
            "_StreamStatus" => s.stream.set(
                d["outputActive"] == true,
                false,
                d["outputDuration"].as_u64(),
            ),
            "RecordStateChanged" => {
                let paused = match output_state {
                    "OBS_WEBSOCKET_OUTPUT_PAUSED" => true,
                    "OBS_WEBSOCKET_OUTPUT_RESUMED" => false,
                    _ => s.record.paused,
                };
                s.record.set(d["outputActive"] == true, paused, None);
            }
            "StreamStateChanged" => s.stream.set(d["outputActive"] == true, false, None),
            "ReplayBufferStateChanged" => s.replay = d["outputActive"] == true,
            "VirtualcamStateChanged" => s.virtualcam = d["outputActive"] == true,
            "StudioModeStateChanged" => s.studio = d["studioModeEnabled"] == true,
            "CurrentSceneTransitionChanged" => s.transition = text(&d["transitionName"]),
            "InputMuteStateChanged" => {
                s.muted
                    .insert(text(&d["inputName"]), d["inputMuted"] == true);
            }
            "InputVolumeChanged" => {
                if let Some(db) = d["inputVolumeDb"].as_f64() {
                    s.volume_db.insert(text(&d["inputName"]), db);
                }
            }
            "SceneItemEnableStateChanged" => {
                s.items.insert(
                    (
                        text(&d["sceneName"]),
                        d["sceneItemId"].as_i64().unwrap_or_default(),
                    ),
                    d["sceneItemEnabled"] == true,
                );
            }
            "SourceFilterEnableStateChanged" => {
                s.filters.insert(
                    (text(&d["sourceName"]), text(&d["filterName"])),
                    d["filterEnabled"] == true,
                );
            }
            "_Stats" => s.stats = d,
            "ExitStarted" => {
                self.obs = None;
                self.state = State::default();
                self.set_status("OBS wurde beendet");
            }
            _ => return,
        }
        self.refresh_all();
    }

    // ------------------------------------------------------------------
    // Key appearance
    // ------------------------------------------------------------------

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        let s = &self.state;
        let on = u16::from;
        let connected = self.obs.is_some();
        if !connected {
            return (
                0,
                if inst.kind() == "connection" {
                    "offline".into()
                } else {
                    String::new()
                },
            );
        }
        match inst.kind() {
            "connection" => (1, String::new()),
            "record" | "stream" => {
                let out = if inst.kind() == "record" {
                    &s.record
                } else {
                    &s.stream
                };
                let label = inst.text_or(
                    "label",
                    if inst.kind() == "record" {
                        "REC"
                    } else {
                        "LIVE"
                    },
                );
                let title = if out.active && inst.flag("timer", true) {
                    let t = format_duration(out.elapsed(), &inst.text_or("format", "hms"));
                    if out.paused { format!("⏸ {t}") } else { t }
                } else {
                    label
                };
                (on(out.active), title)
            }
            "scene" => {
                let scene = inst.text("scene");
                let current = if inst.text("target") == "preview" {
                    &s.preview_scene
                } else {
                    &s.program_scene
                };
                (on(!scene.is_empty() && *current == scene), String::new())
            }
            "sourcevisibility" => {
                let key = (inst.text("scene"), inst.text("source"));
                let enabled = s
                    .item_ids
                    .get(&key)
                    .and_then(|id| s.items.get(&(key.0.clone(), *id)))
                    .copied()
                    .unwrap_or(false);
                (on(enabled), String::new())
            }
            "filter" => (
                on(s.filters
                    .get(&(inst.text("source"), inst.text("filter")))
                    .copied()
                    .unwrap_or(false)),
                String::new(),
            ),
            "audio" => {
                let input = inst.text("input");
                let muted = s.muted.get(&input).copied().unwrap_or(false);
                let title = match s.volume_db.get(&input) {
                    Some(db) if inst.is_encoder() || inst.flag("showVolume", false) => {
                        format_db(*db)
                    }
                    _ => String::new(),
                };
                (on(muted), title)
            }
            "studiomode" => (on(s.studio), String::new()),
            "transition" => (
                on(!s.transition.is_empty() && s.transition == inst.text("transition")),
                String::new(),
            ),
            "performance" => (0, format_stat(&s.stats, &inst.text_or("metric", "cpu"))),
            "replaybuffer" => (on(s.replay), String::new()),
            "virtualcam" => (on(s.virtualcam), String::new()),
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

    /// Every second: running timers; every 2 s: statistics.
    fn on_tick(&mut self, n: u64) {
        if self.obs.is_none() && !self.connecting && n.is_multiple_of(5) {
            self.start_connect();
        }
        let timers = self
            .contexts
            .iter()
            .any(|(_, i)| matches!(i.kind(), "record" | "stream"));
        if timers && (self.state.record.active || self.state.stream.active) {
            self.refresh_all();
        }
        if n.is_multiple_of(2)
            && self.contexts.any("performance")
            && let Some(obs) = self.obs.clone()
        {
            let (tx, generation) = (self.tx.clone(), self.generation);
            tokio::spawn(async move {
                if let Ok(stats) = obs.call("GetStats", Value::Null).await {
                    tx.send(Msg::Obs(
                        generation,
                        ObsEvent::Event("_Stats".into(), stats),
                    ))
                    .ok();
                }
            });
        }
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.refresh(&context);
            self.fetch_for(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            "keyDown" => {
                self.pressed_at.insert(context.clone(), Instant::now());
                // Record/stream decide on release (long press).
                if !self.is_kind(&context, &["record", "stream"]) {
                    self.press(&context, false);
                }
            }
            "keyUp" => {
                if let Some(at) = self.pressed_at.remove(&context)
                    && self.is_kind(&context, &["record", "stream"])
                {
                    let threshold = self
                        .contexts
                        .get(&context)
                        .map(|i| i.number("longPressMs", 650.0))
                        .unwrap_or(650.0);
                    let long = at.elapsed() >= Duration::from_millis(threshold.max(100.0) as u64);
                    self.press(&context, long);
                }
            }
            "dialDown" => self.dial_press(&context),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                self.rotate(&context, ticks);
            }
            _ => {}
        }
    }

    fn is_kind(&self, context: &str, kinds: &[&str]) -> bool {
        self.contexts
            .get(context)
            .is_some_and(|i| kinds.contains(&i.kind()))
    }

    /// Runs requests in the background; failures flash the key.
    fn run(&self, context: &str, requests: Vec<(&'static str, Value)>) {
        let Some(obs) = self.obs.clone() else {
            return;
        };
        let (tx, context) = (self.tx.clone(), context.to_owned());
        tokio::spawn(async move {
            for (request, data) in requests {
                if let Err(err) = obs.call(request, data).await {
                    tx.send(Msg::Failed(Some(context), format!("{request}: {err:#}")))
                        .ok();
                    return;
                }
            }
        });
    }

    fn ensure_connected(&mut self, context: &str) -> bool {
        if self.obs.is_some() {
            return true;
        }
        self.host.alert(context);
        self.start_connect();
        false
    }

    fn press(&mut self, context: &str, long: bool) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        if inst.kind() == "connection" {
            self.disconnect();
            self.start_connect();
            return;
        }
        if !self.ensure_connected(context) {
            return;
        }
        let s = &self.state;
        let toggle = |mode: &str, current: bool| match mode {
            "on" | "start" | "show" => true,
            "off" | "stop" | "hide" => false,
            _ => !current,
        };
        let requests: Vec<(&'static str, Value)> = match inst.kind() {
            "record" => {
                let mode = if long {
                    inst.text_or("longPress", "pause")
                } else {
                    inst.text_or("mode", "toggle")
                };
                match mode.as_str() {
                    "none" => vec![],
                    "start" => vec![("StartRecord", Value::Null)],
                    "stop" => vec![("StopRecord", Value::Null)],
                    "pause" if s.record.active => vec![("ToggleRecordPause", Value::Null)],
                    "pause" => vec![],
                    _ => vec![("ToggleRecord", Value::Null)],
                }
            }
            "stream" => {
                let mode = if long {
                    inst.text_or("longPress", "none")
                } else {
                    inst.text_or("mode", "toggle")
                };
                match mode.as_str() {
                    "none" => vec![],
                    "start" => vec![("StartStream", Value::Null)],
                    "stop" => vec![("StopStream", Value::Null)],
                    _ => vec![("ToggleStream", Value::Null)],
                }
            }
            "scene" => {
                let scene = inst.text("scene");
                if scene.is_empty() {
                    self.host.alert(context);
                    return;
                }
                if inst.text("target") == "preview" && s.studio {
                    vec![("SetCurrentPreviewScene", json!({ "sceneName": scene }))]
                } else {
                    vec![("SetCurrentProgramScene", json!({ "sceneName": scene }))]
                }
            }
            "sourcevisibility" => {
                let key = (inst.text("scene"), inst.text("source"));
                let Some(id) = s.item_ids.get(&key).copied() else {
                    self.host.alert(context);
                    self.fetch_for(context);
                    return;
                };
                let current = s.items.get(&(key.0.clone(), id)).copied().unwrap_or(false);
                let enabled = toggle(&inst.text_or("mode", "toggle"), current);
                vec![(
                    "SetSceneItemEnabled",
                    json!({ "sceneName": key.0, "sceneItemId": id, "sceneItemEnabled": enabled }),
                )]
            }
            "filter" => {
                let (source, filter) = (inst.text("source"), inst.text("filter"));
                let current = s
                    .filters
                    .get(&(source.clone(), filter.clone()))
                    .copied()
                    .unwrap_or(false);
                let enabled = toggle(&inst.text_or("mode", "toggle"), current);
                vec![(
                    "SetSourceFilterEnabled",
                    json!({ "sourceName": source, "filterName": filter, "filterEnabled": enabled }),
                )]
            }
            "audio" => {
                let input = inst.text("input");
                let step = inst.number("step", 2.0);
                match inst.text_or("mode", "mute").as_str() {
                    "up" => return self.change_volume(context, &input, step),
                    "down" => return self.change_volume(context, &input, -step),
                    _ => vec![("ToggleInputMute", json!({ "inputName": input }))],
                }
            }
            "studiomode" => match inst.text_or("mode", "toggle").as_str() {
                "transition" => vec![("TriggerStudioModeTransition", Value::Null)],
                _ => vec![(
                    "SetStudioModeEnabled",
                    json!({ "studioModeEnabled": !s.studio }),
                )],
            },
            "transition" => {
                let mut requests = vec![(
                    "SetCurrentSceneTransition",
                    json!({ "transitionName": inst.text("transition") }),
                )];
                let duration = inst.number("duration", 0.0);
                if duration > 0.0 {
                    requests.push((
                        "SetCurrentSceneTransitionDuration",
                        json!({ "transitionDuration": duration as u64 }),
                    ));
                }
                requests
            }
            "replaybuffer" => match inst.text_or("mode", "save").as_str() {
                "save" if s.replay => vec![("SaveReplayBuffer", Value::Null)],
                "save" => {
                    self.host.alert(context);
                    self.host.log("replay buffer is not running");
                    return;
                }
                "start" => vec![("StartReplayBuffer", Value::Null)],
                "stop" => vec![("StopReplayBuffer", Value::Null)],
                _ => vec![("ToggleReplayBuffer", Value::Null)],
            },
            "virtualcam" => match inst.text_or("mode", "toggle").as_str() {
                "start" => vec![("StartVirtualCam", Value::Null)],
                "stop" => vec![("StopVirtualCam", Value::Null)],
                _ => vec![("ToggleVirtualCam", Value::Null)],
            },
            "macro" => return self.run_macro(context, &inst.text("steps")),
            _ => vec![],
        };
        if inst.kind() == "replaybuffer" && inst.text_or("mode", "save") == "save" {
            self.host.ok(context);
        }
        self.run(context, requests);
    }

    fn dial_press(&mut self, context: &str) {
        match self
            .contexts
            .get(context)
            .map(|i| (i.kind().to_owned(), i.text("input")))
        {
            Some((kind, input)) if kind == "audio" => {
                if self.ensure_connected(context) {
                    self.run(
                        context,
                        vec![("ToggleInputMute", json!({ "inputName": input }))],
                    );
                }
            }
            Some(_) => self.press(context, false),
            None => {}
        }
    }

    fn rotate(&mut self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        if ticks == 0 || inst.kind() != "audio" || !self.ensure_connected(context) {
            return;
        }
        let step = inst.number("step", 2.0);
        self.change_volume(context, &inst.text("input"), step * ticks as f64);
    }

    fn change_volume(&mut self, context: &str, input: &str, delta_db: f64) {
        let current = self.state.volume_db.get(input).copied().unwrap_or(0.0);
        // OBS shows -inf below -100 dB; 26 dB is the maximum.
        let db = ((current.max(-100.0) + delta_db).clamp(-100.0, 26.0) * 10.0).round() / 10.0;
        self.state.volume_db.insert(input.to_owned(), db);
        self.run(
            context,
            vec![(
                "SetInputVolume",
                json!({ "inputName": input, "inputVolumeDb": db }),
            )],
        );
        self.refresh_all();
    }

    /// One step per line: `RequestType {json}` or `wait 500`.
    fn run_macro(&self, context: &str, steps: &str) {
        let steps = match parse_macro(steps) {
            Ok(steps) if !steps.is_empty() => steps,
            Ok(_) => {
                self.host.alert(context);
                return;
            }
            Err(err) => {
                self.host.alert(context);
                self.host.log(format!("macro: {err:#}"));
                return;
            }
        };
        let Some(obs) = self.obs.clone() else {
            return;
        };
        let (tx, context) = (self.tx.clone(), context.to_owned());
        tokio::spawn(async move {
            for step in steps {
                match step {
                    Step::Wait(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                    Step::Request(request, data) => {
                        if let Err(err) = obs.call(&request, data).await {
                            tx.send(Msg::Failed(Some(context), format!("{request}: {err:#}")))
                                .ok();
                            return;
                        }
                    }
                }
            }
        });
    }

    /// Dropdown options for the settings forms.
    fn on_request(&self, req: Value) {
        let (host, obs) = (self.host.clone(), self.obs.clone());
        tokio::spawn(async move {
            let options = match obs {
                None => Err(anyhow::anyhow!(
                    "Nicht mit OBS verbunden – Plugins → OBS Studio → Einstellungen"
                )),
                Some(obs) => options(&obs, &req).await,
            };
            host.reply_options(&req, options);
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(value) => self.on_host(value),
            Msg::Obs(generation, event) => self.on_obs(generation, event),
            Msg::Connected(generation, result) => self.on_connected(generation, result),
            Msg::ItemId(generation, scene, source, id) => {
                if generation == self.generation {
                    self.state.item_ids.insert((scene, source), id);
                    self.refresh_all();
                }
            }
            Msg::Failed(context, error) => {
                self.host.log(error);
                if let Some(context) = context {
                    self.host.alert(&context);
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Step {
    Wait(u64),
    Request(String, Value),
}

fn parse_macro(text: &str) -> anyhow::Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let (head, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        if matches!(head.to_lowercase().as_str(), "wait" | "warte" | "pause") {
            let ms = rest
                .trim()
                .trim_end_matches("ms")
                .trim()
                .parse()
                .map_err(|_| {
                    anyhow::anyhow!("Zeile {}: „{head} <Millisekunden>“ erwartet", n + 1)
                })?;
            steps.push(Step::Wait(ms));
        } else {
            let data = if rest.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(rest.trim())
                    .map_err(|e| anyhow::anyhow!("Zeile {}: JSON ungültig ({e})", n + 1))?
            };
            steps.push(Step::Request(head.to_owned(), data));
        }
    }
    Ok(steps)
}

fn format_duration(d: Duration, format: &str) -> String {
    let s = d.as_secs();
    match format {
        "ms" => format!("{:02}:{:02}", s / 60, s % 60),
        _ => format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60),
    }
}

fn format_db(db: f64) -> String {
    if db <= -100.0 {
        "-∞ dB".into()
    } else {
        format!("{db:.1} dB").replace('.', ",")
    }
}

fn format_stat(stats: &Value, metric: &str) -> String {
    let num = |k: &str| stats[k].as_f64();
    let pct = |a: &str, b: &str| match (num(a), num(b)) {
        (Some(x), Some(total)) if total > 0.0 => Some(x / total * 100.0),
        (Some(_), _) => Some(0.0),
        _ => None,
    };
    let value = match metric {
        "fps" => num("activeFps").map(|v| format!("{v:.0} fps")),
        "memory" => num("memoryUsage").map(|v| format!("{v:.0} MB")),
        "render" => {
            pct("renderSkippedFrames", "renderTotalFrames").map(|v| format!("Render\n{v:.1} %"))
        }
        "output" => {
            pct("outputSkippedFrames", "outputTotalFrames").map(|v| format!("Output\n{v:.1} %"))
        }
        "disk" => num("availableDiskSpace").map(|v| format!("{:.0} GB", v / 1024.0)),
        _ => num("cpuUsage").map(|v| format!("CPU\n{v:.1} %")),
    };
    value.map(|v| v.replace('.', ",")).unwrap_or_default()
}

async fn options(obs: &Obs, req: &Value) -> anyhow::Result<Vec<(String, String)>> {
    let names = |v: &Value, list: &str, key: &str| -> Vec<(String, String)> {
        v[list]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e[key].as_str())
            .map(|n| (n.to_owned(), n.to_owned()))
            .collect()
    };
    let text = |k: &str| req[k].as_str().unwrap_or_default().to_owned();
    Ok(match req["request"].as_str().unwrap_or_default() {
        "scenes" => {
            // OBS lists scenes bottom-up; show them like the scene dock.
            let mut list = names(
                &obs.call("GetSceneList", Value::Null).await?,
                "scenes",
                "sceneName",
            );
            list.reverse();
            list
        }
        "sceneItems" => {
            let scene = text("scene");
            anyhow::ensure!(!scene.is_empty(), "Erst eine Szene wählen");
            let mut list = names(
                &obs.call("GetSceneItemList", json!({ "sceneName": scene }))
                    .await?,
                "sceneItems",
                "sourceName",
            );
            list.reverse();
            list
        }
        "sources" => {
            let mut list = names(
                &obs.call("GetInputList", Value::Null).await?,
                "inputs",
                "inputName",
            );
            let mut scenes = names(
                &obs.call("GetSceneList", Value::Null).await?,
                "scenes",
                "sceneName",
            );
            scenes.reverse();
            for (value, label) in scenes {
                list.push((value, format!("{label} (Szene)")));
            }
            list
        }
        "filters" => {
            let source = text("source");
            anyhow::ensure!(!source.is_empty(), "Erst eine Quelle wählen");
            names(
                &obs.call("GetSourceFilterList", json!({ "sourceName": source }))
                    .await?,
                "filters",
                "filterName",
            )
        }
        "audioInputs" => {
            let inputs = obs.call("GetInputList", Value::Null).await?;
            let mut list = Vec::new();
            for input in inputs["inputs"].as_array().into_iter().flatten() {
                let name = input["inputName"].as_str().unwrap_or_default();
                // Only sources with an audio track answer GetInputVolume.
                if obs
                    .call("GetInputVolume", json!({ "inputName": name }))
                    .await
                    .is_ok()
                {
                    list.push((name.to_owned(), name.to_owned()));
                }
            }
            list
        }
        "transitions" => names(
            &obs.call("GetSceneTransitionList", Value::Null).await?,
            "transitions",
            "transitionName",
        ),
        other => anyhow::bail!("unbekannte Anfrage {other}"),
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut n = 0u64;
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.handle(Msg::Host(msg)),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.handle(msg),
            _ = tick.tick() => {
                n += 1;
                plugin.on_tick(n);
            }
        }
    }
    if let Some(obs) = plugin.obs.take() {
        obs.close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    async fn connected(
        respond: impl Fn(&str, &Value) -> Value + Send + Sync + 'static,
    ) -> (
        Plugin,
        mpsc::UnboundedReceiver<Value>,
        mpsc::UnboundedReceiver<Msg>,
        Arc<Mutex<Vec<Value>>>,
    ) {
        let (port, log) = obs::tests::fake_obs(respond).await;
        let (host, host_rx) = Host::channel("de.opendeckn3.obs");
        let (tx, rx) = mpsc::unbounded_channel();
        let mut p = Plugin::new(host, tx);
        let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
            password: "pw".into(),
        };
        p.obs = Some(Obs::connect(&endpoint, ev_tx).await.unwrap());
        (p, host_rx, rx, log)
    }

    fn appear(p: &mut Plugin, context: &str, kind: &str, settings: Value) {
        p.on_host(json!({ "event": "willAppear", "context": context, "action": format!("de.opendeckn3.obs.{kind}"),
            "payload": { "controller": "Keypad", "settings": settings } }));
    }

    async fn requests(log: &Arc<Mutex<Vec<Value>>>, n: usize) -> Vec<(String, Value)> {
        for _ in 0..200 {
            let reqs: Vec<(String, Value)> = log
                .lock()
                .unwrap()
                .iter()
                .filter(|m| m["op"] == 6)
                .map(|m| {
                    (
                        m["d"]["requestType"].as_str().unwrap().to_owned(),
                        m["d"]["requestData"].clone(),
                    )
                })
                .collect();
            if reqs.len() >= n {
                return reqs;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {n} requests: {:?}", log.lock().unwrap());
    }

    #[tokio::test]
    async fn record_short_and_long_press() {
        let (mut p, _host, _rx, log) = connected(|_, _| json!({})).await;
        appear(
            &mut p,
            "r",
            "record",
            json!({ "mode": "toggle", "longPress": "stop", "longPressMs": 100 }),
        );
        p.on_host(json!({ "event": "keyDown", "context": "r" }));
        p.on_host(json!({ "event": "keyUp", "context": "r" }));
        p.on_host(json!({ "event": "keyDown", "context": "r" }));
        tokio::time::sleep(Duration::from_millis(150)).await;
        p.on_host(json!({ "event": "keyUp", "context": "r" }));
        let reqs = requests(&log, 2).await;
        assert_eq!(reqs[0].0, "ToggleRecord");
        assert_eq!(reqs[1].0, "StopRecord");
    }

    #[tokio::test]
    async fn scene_state_and_switch() {
        let (mut p, mut host, _rx, log) = connected(|_, _| json!({})).await;
        appear(&mut p, "s", "scene", json!({ "scene": "Kamera" }));
        let g = p.generation;
        p.on_obs(
            g,
            ObsEvent::Event(
                "CurrentProgramSceneChanged".into(),
                json!({ "sceneName": "Kamera" }),
            ),
        );
        let states: Vec<u64> = std::iter::from_fn(|| host.try_recv().ok())
            .filter(|m| m["event"] == "setState")
            .map(|m| m["payload"]["state"].as_u64().unwrap())
            .collect();
        assert_eq!(states.last(), Some(&1), "active scene is highlighted");
        p.on_host(json!({ "event": "keyDown", "context": "s" }));
        let reqs = requests(&log, 1).await;
        assert_eq!(
            reqs[0],
            (
                "SetCurrentProgramScene".to_owned(),
                json!({ "sceneName": "Kamera" })
            )
        );
    }

    #[tokio::test]
    async fn audio_dial_changes_volume_and_press_mutes() {
        let (mut p, _host, _rx, log) = connected(|_, _| json!({})).await;
        p.on_host(
            json!({ "event": "willAppear", "context": "a", "action": "de.opendeckn3.obs.audio",
            "payload": { "controller": "Encoder", "settings": { "input": "Mikro", "step": 2 } } }),
        );
        p.state.volume_db.insert("Mikro".into(), -10.0);
        p.on_host(json!({ "event": "dialRotate", "context": "a", "payload": { "ticks": 3 } }));
        p.on_host(json!({ "event": "dialDown", "context": "a" }));
        let reqs = requests(&log, 4).await;
        let set = reqs.iter().find(|r| r.0 == "SetInputVolume").unwrap();
        assert_eq!(
            set.1,
            json!({ "inputName": "Mikro", "inputVolumeDb": -4.0 })
        );
        assert!(reqs.iter().any(|r| r.0 == "ToggleInputMute"));
    }

    #[tokio::test]
    async fn dropdowns_list_scenes_and_items() {
        let (p, _host, _rx, _log) = connected(|kind, data| match kind {
            "GetSceneList" => {
                json!({ "scenes": [{ "sceneName": "Unten" }, { "sceneName": "Oben" }] })
            }
            "GetSceneItemList" => {
                assert_eq!(data["sceneName"], "Oben");
                json!({ "sceneItems": [{ "sourceName": "Webcam" }] })
            }
            _ => json!({}),
        })
        .await;
        let obs = p.obs.clone().unwrap();
        let scenes = options(&obs, &json!({ "request": "scenes" }))
            .await
            .unwrap();
        assert_eq!(scenes[0].0, "Oben", "same order as in OBS");
        let items = options(&obs, &json!({ "request": "sceneItems", "scene": "Oben" }))
            .await
            .unwrap();
        assert_eq!(items, [("Webcam".to_owned(), "Webcam".to_owned())]);
    }

    #[test]
    fn macro_parsing() {
        let steps = parse_macro(
            "# Intro\nSetCurrentProgramScene {\"sceneName\":\"Intro\"}\nwait 1500\nStartRecord",
        )
        .unwrap();
        assert_eq!(
            steps,
            [
                Step::Request(
                    "SetCurrentProgramScene".into(),
                    json!({ "sceneName": "Intro" })
                ),
                Step::Wait(1500),
                Step::Request("StartRecord".into(), Value::Null),
            ]
        );
        assert!(parse_macro("warte abc").is_err());
        assert!(parse_macro("SetX {kaputt").is_err());
    }

    #[test]
    fn timer_follows_pause_and_resume() {
        let mut out = Output::default();
        out.set(true, false, Some(61_000));
        assert_eq!(format_duration(out.elapsed(), "hms"), "00:01:01");
        out.set(true, true, None);
        let paused_at = out.elapsed();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(out.elapsed(), paused_at, "clock stops while paused");
        out.set(false, false, None);
        assert_eq!(out.elapsed(), Duration::ZERO);
        assert_eq!(format_duration(Duration::from_secs(3725), "ms"), "62:05");
    }

    #[test]
    fn stats_and_db_text() {
        let stats = json!({ "cpuUsage": 3.12, "activeFps": 60.0, "renderSkippedFrames": 1, "renderTotalFrames": 200 });
        assert_eq!(format_stat(&stats, "cpu"), "CPU\n3,1 %");
        assert_eq!(format_stat(&stats, "fps"), "60 fps");
        assert_eq!(format_stat(&stats, "render"), "Render\n0,5 %");
        assert_eq!(format_db(-6.0), "-6,0 dB");
        assert_eq!(format_db(-100.0), "-∞ dB");
    }
}

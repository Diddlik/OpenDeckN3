#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 Home Assistant plugin: webhooks, service calls, switches,
//! dimmers and state displays over the Home Assistant REST API
//! (see `docs/PLUGIN_HOMEASSISTANT.md`).

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, anyhow, bail};
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

/// States that count as "on" for the key highlight.
const ON_STATES: [&str; 6] = ["on", "open", "playing", "home", "unlocked", "active"];
/// Domains that `homeassistant.toggle` can switch.
const TOGGLE_DOMAINS: [&str; 10] = [
    "light",
    "switch",
    "fan",
    "input_boolean",
    "cover",
    "media_player",
    "lock",
    "automation",
    "script",
    "climate",
];
const DIMMABLE_DOMAINS: [&str; 5] = ["light", "media_player", "input_number", "fan", "cover"];
const CUSTOM: &str = "__custom";

/// Home Assistant REST client.
#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Api {
    fn new(base: &str, token: &str) -> anyhow::Result<Self> {
        let base = base.trim().trim_end_matches('/');
        anyhow::ensure!(
            base.starts_with("http://") || base.starts_with("https://"),
            "Adresse von Home Assistant eintragen, z. B. http://homeassistant.local:8123"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!(
                    "OpenDeckN3-HomeAssistant/",
                    env!("CARGO_PKG_VERSION")
                ))
                .timeout(Duration::from_secs(10))
                .build()?,
            base: base.to_owned(),
            token: token.trim().to_owned(),
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            !self.token.is_empty(),
            "Zugriffstoken fehlt (Plugins → Home Assistant → Einstellungen)"
        );
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req
            .send()
            .await
            .map_err(|e| anyhow!("Home Assistant nicht erreichbar ({}): {e}", self.base))?;
        match res.status().as_u16() {
            200..=299 => {}
            401 | 403 => bail!("Zugriffstoken ungültig"),
            404 => bail!("nicht gefunden: {path}"),
            code => bail!("Home Assistant antwortet mit HTTP {code}"),
        }
        let bytes = res.bytes().await?;
        Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }

    async fn call_service(&self, service: &str, data: Value) -> anyhow::Result<Value> {
        let (domain, name) = service
            .split_once('.')
            .ok_or_else(|| anyhow!("Dienst als „domain.dienst“ angeben, z. B. light.toggle"))?;
        self.request(
            reqwest::Method::POST,
            &format!("/api/services/{domain}/{name}"),
            Some(data),
        )
        .await
    }

    /// Webhooks need no token; the id is the secret.
    async fn webhook(&self, id: &str, method: &str, payload: &str) -> anyhow::Result<()> {
        let method = match method {
            "PUT" => reqwest::Method::PUT,
            "GET" => reqwest::Method::GET,
            _ => reqwest::Method::POST,
        };
        let mut req = self
            .http
            .request(method, format!("{}/api/webhook/{}", self.base, id.trim()));
        let payload = payload.trim();
        if !payload.is_empty() {
            req = match serde_json::from_str::<Value>(payload) {
                Ok(json) => req.json(&json),
                Err(_) => req
                    .header("content-type", "text/plain")
                    .body(payload.to_owned()),
            };
        }
        let res = req
            .send()
            .await
            .map_err(|e| anyhow!("Home Assistant nicht erreichbar ({}): {e}", self.base))?;
        anyhow::ensure!(
            res.status().is_success(),
            "Webhook abgelehnt (HTTP {})",
            res.status().as_u16()
        );
        Ok(())
    }
}

enum Msg {
    Host(Value),
    States(u64, anyhow::Result<Value>),
    Status(u64, anyhow::Result<String>),
    Failed(String, String),
    Done(String),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    api: Option<Api>,
    generation: u64,
    states: HashMap<String, Value>,
    polling: bool,
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
            states: HashMap::new(),
            polling: false,
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
        if !(self.api.is_none() || ["url", "token", "connect"].into_iter().any(changed)) {
            return;
        }
        self.generation += 1;
        self.states.clear();
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
        if let Some(api) = self.api.clone() {
            self.set_status("Verbinde mit Home Assistant …");
            let (tx, generation) = (self.tx.clone(), self.generation);
            tokio::spawn(async move {
                let result = async {
                    let info = api.get("/api/config").await?;
                    let name = info["location_name"].as_str().unwrap_or("Home Assistant");
                    let version = info["version"].as_str().unwrap_or_default();
                    Ok(format!("Verbunden mit {name} {version}").trim().to_owned())
                }
                .await;
                tx.send(Msg::Status(generation, result)).ok();
            });
            self.poll();
        }
        self.refresh_all();
    }

    /// Defined webhooks from the global settings: `Name = id` per line.
    fn webhooks(&self) -> Vec<(String, String)> {
        parse_webhooks(&global_str(&self.global, "webhooks"))
    }

    fn needs_states(&self) -> bool {
        self.contexts
            .iter()
            .any(|(_, i)| matches!(i.kind(), "toggle" | "dimmer" | "state"))
    }

    fn poll(&mut self) {
        let Some(api) = self.api.clone() else {
            return;
        };
        if self.polling || !self.needs_states() {
            return;
        }
        self.polling = true;
        let (tx, generation) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            tx.send(Msg::States(generation, api.get("/api/states").await))
                .ok();
        });
    }

    fn on_states(&mut self, generation: u64, result: anyhow::Result<Value>) {
        self.polling = false;
        if generation != self.generation {
            return;
        }
        match result {
            Ok(Value::Array(list)) => {
                self.states = list
                    .into_iter()
                    .filter_map(|s| Some((s["entity_id"].as_str()?.to_owned(), s)))
                    .collect();
                self.refresh_all();
            }
            Ok(_) => {}
            Err(err) => self.set_status(format!("Nicht verbunden: {err:#}")),
        }
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        let entity = inst.text("entity");
        let Some(state) = self.states.get(&entity) else {
            return (0, String::new());
        };
        let value = state["state"].as_str().unwrap_or_default();
        let on = u16::from(ON_STATES.contains(&value));
        let attrs = &state["attributes"];
        match inst.kind() {
            "toggle" => (
                on,
                if inst.flag("showState", false) {
                    state_text(state)
                } else {
                    String::new()
                },
            ),
            "dimmer" => (on, level_text(&entity, attrs, value)),
            "state" => (on, state_text(state)),
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

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.refresh(&context);
            self.poll();
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                self.rotate(&context, ticks);
            }
            _ => {}
        }
    }

    /// Runs an API call in the background; result flashes the key.
    fn spawn(
        &self,
        context: &str,
        job: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        let (tx, context) = (self.tx.clone(), context.to_owned());
        tokio::spawn(async move {
            match job.await {
                Ok(()) => tx.send(Msg::Done(context)).ok(),
                Err(err) => tx.send(Msg::Failed(context, format!("{err:#}"))).ok(),
            };
        });
    }

    fn press(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        let Some(api) = self.api.clone() else {
            self.host.alert(context);
            return;
        };
        match inst.kind() {
            "webhook" => {
                let id = match inst.text("webhook").as_str() {
                    CUSTOM | "" => inst.text("webhookId"),
                    chosen => chosen.to_owned(),
                };
                if id.is_empty() {
                    self.host.alert(context);
                    return;
                }
                let (method, payload) = (inst.text_or("method", "POST"), inst.text("payload"));
                self.spawn(
                    context,
                    async move { api.webhook(&id, &method, &payload).await },
                );
            }
            "service" => {
                let service = inst.text("service");
                let data = match service_data(&inst) {
                    Ok(data) => data,
                    Err(err) => {
                        self.host.alert(context);
                        self.host.log(format!("{err:#}"));
                        return;
                    }
                };
                self.spawn(context, async move {
                    api.call_service(&service, data).await.map(drop)
                });
            }
            "toggle" | "dimmer" | "state" => {
                let entity = inst.text("entity");
                if entity.is_empty() {
                    self.host.alert(context);
                    return;
                }
                if inst.kind() == "state" {
                    self.poll();
                    return;
                }
                self.spawn(context, async move {
                    api.call_service("homeassistant.toggle", json!({ "entity_id": entity }))
                        .await
                        .map(drop)
                });
            }
            _ => {}
        }
    }

    fn rotate(&mut self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        let Some(api) = self.api.clone() else {
            self.host.alert(context);
            return;
        };
        if inst.kind() != "dimmer" || ticks == 0 {
            return;
        }
        let entity = inst.text("entity");
        let step = inst.number("step", 10.0);
        match dimmer_call(&entity, self.states.get(&entity), step * ticks as f64) {
            Ok((service, data, optimistic)) => {
                // Show the new level right away; the next poll confirms it.
                if let (Some(state), Some((key, value))) =
                    (self.states.get_mut(&entity), optimistic)
                {
                    state["attributes"][key] = value;
                }
                self.refresh_all();
                self.spawn(context, async move {
                    api.call_service(&service, data).await.map(drop)
                });
            }
            Err(err) => {
                self.host.alert(context);
                self.host.log(format!("{err:#}"));
            }
        }
    }

    fn on_request(&self, req: Value) {
        let host = self.host.clone();
        let api = self.api.clone();
        let webhooks = self.webhooks();
        tokio::spawn(async move {
            let result = options(api.as_ref(), &webhooks, &req).await;
            host.reply_options(&req, result);
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::States(generation, result) => self.on_states(generation, result),
            Msg::Status(generation, result) => {
                if generation == self.generation {
                    match result {
                        Ok(status) => self.set_status(status),
                        Err(err) => self.set_status(format!("Nicht verbunden: {err:#}")),
                    }
                }
            }
            Msg::Failed(context, error) => {
                self.host.alert(&context);
                self.host.log(error);
            }
            Msg::Done(context) => {
                self.host.ok(&context);
                // Pick up the new state soon.
                let tx = self.tx.clone();
                let generation = self.generation;
                if let Some(api) = self.api.clone().filter(|_| self.needs_states()) {
                    self.polling = true;
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        tx.send(Msg::States(generation, api.get("/api/states").await))
                            .ok();
                    });
                }
            }
        }
    }
}

/// `Name = id` per line (or just the id).
fn parse_webhooks(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| match line.split_once('=') {
            Some((name, id)) => (id.trim().to_owned(), name.trim().to_owned()),
            None => (line.to_owned(), line.to_owned()),
        })
        .filter(|(id, _)| !id.is_empty())
        .collect()
}

/// Service data: JSON from the settings plus the chosen entity.
fn service_data(inst: &Instance) -> anyhow::Result<Value> {
    anyhow::ensure!(!inst.text("service").is_empty(), "kein Dienst gewählt");
    let text = inst.text("data");
    let mut data = if text.is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(&text).context("Daten sind kein gültiges JSON")?
    };
    anyhow::ensure!(
        data.is_object(),
        "Daten müssen ein JSON-Objekt sein, z. B. {{\"brightness_pct\": 50}}"
    );
    let entity = inst.text("entity");
    if !entity.is_empty() && data.get("entity_id").is_none() {
        data["entity_id"] = json!(entity);
    }
    Ok(data)
}

/// Service, its data, and an attribute to update optimistically.
type DimmerCall = (String, Value, Option<(&'static str, Value)>);

/// Service call for turning a dimmer knob by `delta` percent; also returns
/// the attribute to update optimistically.
fn dimmer_call(entity: &str, state: Option<&Value>, delta: f64) -> anyhow::Result<DimmerCall> {
    let domain = entity.split('.').next().unwrap_or_default();
    let attrs = state.map(|s| &s["attributes"]).cloned().unwrap_or_default();
    let num = |k: &str| attrs[k].as_f64();
    Ok(match domain {
        "light" => {
            let current = num("brightness").unwrap_or(0.0) / 255.0 * 100.0;
            let target = (current + delta).clamp(0.0, 100.0);
            (
                "light.turn_on".into(),
                json!({ "entity_id": entity, "brightness_step_pct": delta.round() as i64 }),
                Some(("brightness", json!((target / 100.0 * 255.0).round()))),
            )
        }
        "media_player" => {
            let level = (num("volume_level").unwrap_or(0.0) + delta / 100.0).clamp(0.0, 1.0);
            let level = (level * 100.0).round() / 100.0;
            (
                "media_player.volume_set".into(),
                json!({ "entity_id": entity, "volume_level": level }),
                Some(("volume_level", json!(level))),
            )
        }
        "fan" => {
            let pct = (num("percentage").unwrap_or(0.0) + delta)
                .clamp(0.0, 100.0)
                .round();
            (
                "fan.set_percentage".into(),
                json!({ "entity_id": entity, "percentage": pct }),
                Some(("percentage", json!(pct))),
            )
        }
        "cover" => {
            let pos = (num("current_position").unwrap_or(0.0) + delta)
                .clamp(0.0, 100.0)
                .round();
            (
                "cover.set_cover_position".into(),
                json!({ "entity_id": entity, "position": pos }),
                Some(("current_position", json!(pos))),
            )
        }
        "input_number" => {
            let current: f64 = state
                .and_then(|s| s["state"].as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let (min, max) = (
                num("min").unwrap_or(f64::MIN),
                num("max").unwrap_or(f64::MAX),
            );
            // Knob step = setting in units of the helper's own step size.
            let unit = num("step").unwrap_or(1.0);
            let value = (current + delta / 10.0 * unit).clamp(min, max);
            (
                "input_number.set_value".into(),
                json!({ "entity_id": entity, "value": value }),
                None,
            )
        }
        _ => bail!(
            "{entity} lässt sich nicht dimmen (Licht, Mediaplayer, Lüfter, Rollladen oder input_number)"
        ),
    })
}

/// Title for a dimmer: brightness/volume/position in percent.
fn level_text(entity: &str, attrs: &Value, state: &str) -> String {
    let pct = |v: f64| format!("{} %", v.round());
    match entity.split('.').next().unwrap_or_default() {
        "light" if state == "on" => attrs["brightness"]
            .as_f64()
            .map(|b| pct(b / 255.0 * 100.0))
            .unwrap_or_default(),
        "light" => "aus".into(),
        "media_player" => attrs["volume_level"]
            .as_f64()
            .map(|v| pct(v * 100.0))
            .unwrap_or_default(),
        "fan" => attrs["percentage"].as_f64().map(pct).unwrap_or_default(),
        "cover" => attrs["current_position"]
            .as_f64()
            .map(pct)
            .unwrap_or_default(),
        _ => state.to_owned(),
    }
}

/// `21,5 °C`, `an`, `aus` …
fn state_text(state: &Value) -> String {
    let value = state["state"].as_str().unwrap_or_default();
    let unit = state["attributes"]["unit_of_measurement"]
        .as_str()
        .unwrap_or_default();
    let translated = match value {
        "on" => "an",
        "off" => "aus",
        "open" => "offen",
        "closed" => "zu",
        "locked" => "zu",
        "unlocked" => "offen",
        "playing" => "spielt",
        "paused" => "Pause",
        "home" => "zuhause",
        "not_home" => "weg",
        "unavailable" => "n. v.",
        other => other,
    };
    let text = if value.parse::<f64>().is_ok() {
        value.replace('.', ",")
    } else {
        translated.to_owned()
    };
    if unit.is_empty() {
        text
    } else {
        format!("{text} {unit}")
    }
}

async fn options(
    api: Option<&Api>,
    webhooks: &[(String, String)],
    req: &Value,
) -> anyhow::Result<Vec<(String, String)>> {
    let kind = req["request"].as_str().unwrap_or_default();
    if kind == "webhooks" {
        let mut list = webhooks.to_vec();
        list.push((CUSTOM.into(), "Andere Webhook-ID …".into()));
        return Ok(list);
    }
    let api = api.ok_or_else(|| {
        anyhow!("Home Assistant ist nicht eingerichtet (Plugins → Home Assistant)")
    })?;
    match kind {
        "services" => {
            let data = api.get("/api/services").await?;
            let mut list: Vec<(String, String)> = Vec::new();
            for domain in data.as_array().into_iter().flatten() {
                let d = domain["domain"].as_str().unwrap_or_default();
                for (name, info) in domain["services"].as_object().into_iter().flatten() {
                    let id = format!("{d}.{name}");
                    let label = match info["name"].as_str() {
                        Some(n) if !n.is_empty() => format!("{id} – {n}"),
                        _ => id.clone(),
                    };
                    list.push((id, label));
                }
            }
            list.sort();
            Ok(list)
        }
        "entities" | "toggleable" | "dimmable" => {
            let domains: Vec<String> = match kind {
                "toggleable" => TOGGLE_DOMAINS.iter().map(|d| d.to_string()).collect(),
                "dimmable" => DIMMABLE_DOMAINS.iter().map(|d| d.to_string()).collect(),
                // Entities of the chosen service's domain (any for homeassistant.*).
                _ => match req["service"].as_str().and_then(|s| s.split_once('.')) {
                    Some((d, _)) if d != "homeassistant" => vec![d.to_owned()],
                    _ => vec![],
                },
            };
            let states = api.get("/api/states").await?;
            let mut list: Vec<(String, String)> = states
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| {
                    let id = s["entity_id"].as_str()?;
                    let domain = id.split('.').next()?;
                    if !domains.is_empty() && !domains.iter().any(|d| d == domain) {
                        return None;
                    }
                    let name = s["attributes"]["friendly_name"].as_str().unwrap_or(id);
                    Some((id.to_owned(), format!("{name} ({id})")))
                })
                .collect();
            list.sort_by_key(|a| a.1.to_lowercase());
            if kind == "entities" && !domains.is_empty() {
                list.insert(0, (String::new(), "– keine Entität –".into()));
            }
            Ok(list)
        }
        other => bail!("unbekannte Anfrage {other}"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx);
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.handle(Msg::Host(msg)),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.handle(msg),
            _ = tick.tick() => plugin.poll(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal Home Assistant: records requests, answers `respond(path)`.
    async fn fake_ha(respond: fn(&str) -> (u16, Value)) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let mut n = sock.read(&mut buf).await.unwrap();
                // Body may come in a second packet.
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some(len) = head.lines().find_map(|l| {
                    l.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                }) {
                    let body_start = head.find("\r\n\r\n").unwrap() + 4;
                    while n < body_start + len {
                        n += sock.read(&mut buf[n..]).await.unwrap();
                    }
                }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or_default().to_owned();
                log2.lock().unwrap().push(req);
                let (code, body) = respond(&path);
                let body = body.to_string();
                let res = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(res.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{addr}"), log)
    }

    fn states(_: &str) -> (u16, Value) {
        (
            200,
            json!([
                { "entity_id": "light.buero", "state": "on", "attributes": { "friendly_name": "Büro", "brightness": 128 } },
                { "entity_id": "sensor.temp", "state": "21.5", "attributes": { "friendly_name": "Temperatur", "unit_of_measurement": "°C" } },
                { "entity_id": "switch.kaffee", "state": "off", "attributes": { "friendly_name": "Kaffeemaschine" } }
            ]),
        )
    }

    fn plugin(
        base: &str,
    ) -> (
        Plugin,
        mpsc::UnboundedReceiver<Value>,
        mpsc::UnboundedReceiver<Msg>,
    ) {
        let (host, host_rx) = Host::channel("de.opendeckn3.homeassistant");
        let (tx, rx) = mpsc::unbounded_channel();
        let mut p = Plugin::new(host, tx);
        p.global = json!({ "url": base, "token": "T", "webhooks": "Licht an = licht_an\n# Kommentar\nalarm" })
            .as_object()
            .unwrap()
            .clone();
        p.api = Some(Api::new(base, "T").unwrap());
        (p, host_rx, rx)
    }

    fn appear(p: &mut Plugin, context: &str, kind: &str, settings: Value) {
        p.on_host(json!({ "event": "willAppear", "context": context, "action": format!("de.opendeckn3.homeassistant.{kind}"),
            "payload": { "controller": "Keypad", "settings": settings } }));
    }

    async fn next_done(rx: &mut mpsc::UnboundedReceiver<Msg>) -> Result<String, String> {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                Msg::Done(c) => return Ok(c),
                Msg::Failed(_, e) => return Err(e),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn webhook_posts_payload_without_token() {
        let (base, log) = fake_ha(|_| (200, Value::Null)).await;
        let (mut p, _host, mut rx) = plugin(&base);
        appear(
            &mut p,
            "w",
            "webhook",
            json!({ "webhook": "licht_an", "payload": "{\"szene\": \"abend\"}" }),
        );
        p.on_host(json!({ "event": "keyDown", "context": "w" }));
        assert_eq!(next_done(&mut rx).await.unwrap(), "w");
        let req = log.lock().unwrap().last().unwrap().clone();
        assert!(req.starts_with("POST /api/webhook/licht_an "), "{req}");
        assert!(req.contains("{\"szene\":\"abend\"}"), "{req}");
        assert!(
            !req.to_lowercase().contains("authorization"),
            "webhooks are sent without the token"
        );
    }

    #[tokio::test]
    async fn service_call_with_entity_and_data() {
        let (base, log) = fake_ha(|_| (200, json!([]))).await;
        let (mut p, _host, mut rx) = plugin(&base);
        appear(
            &mut p,
            "s",
            "service",
            json!({ "service": "light.turn_on", "entity": "light.buero", "data": "{\"brightness_pct\": 40}" }),
        );
        p.on_host(json!({ "event": "keyDown", "context": "s" }));
        next_done(&mut rx).await.unwrap();
        let req = log.lock().unwrap().last().unwrap().clone();
        assert!(
            req.starts_with("POST /api/services/light/turn_on "),
            "{req}"
        );
        assert!(req.contains("Bearer T"), "{req}");
        assert!(
            req.contains("\"entity_id\":\"light.buero\"") && req.contains("\"brightness_pct\":40"),
            "{req}"
        );
    }

    #[tokio::test]
    async fn states_drive_key_display() {
        let (base, _log) = fake_ha(states).await;
        let (mut p, mut host, mut rx) = plugin(&base);
        appear(&mut p, "t", "toggle", json!({ "entity": "light.buero" }));
        appear(&mut p, "z", "state", json!({ "entity": "sensor.temp" }));
        let msg = loop {
            if let m @ Msg::States(..) = rx.recv().await.unwrap() {
                break m;
            }
        };
        p.handle(msg);
        let sent: Vec<Value> = std::iter::from_fn(|| host.try_recv().ok()).collect();
        assert!(sent.iter().any(|m| m["event"] == "setState"
            && m["context"] == "t"
            && m["payload"]["state"] == 1));
        assert!(sent.iter().any(|m| m["event"] == "setTitle"
            && m["context"] == "z"
            && m["payload"]["title"] == "21,5 °C"));
    }

    #[tokio::test]
    async fn dropdowns() {
        let (base, _log) = fake_ha(|path| match path {
            "/api/services" => (
                200,
                json!([{ "domain": "light", "services": { "toggle": { "name": "Umschalten" } } }]),
            ),
            _ => states(path),
        })
        .await;
        let (p, _host, _rx) = plugin(&base);
        let hooks = options(
            p.api.as_ref(),
            &p.webhooks(),
            &json!({ "request": "webhooks" }),
        )
        .await
        .unwrap();
        assert_eq!(hooks[0], ("licht_an".to_owned(), "Licht an".to_owned()));
        assert_eq!(hooks[1].0, "alarm");
        assert_eq!(hooks.last().unwrap().0, CUSTOM);
        let services = options(p.api.as_ref(), &[], &json!({ "request": "services" }))
            .await
            .unwrap();
        assert_eq!(
            services,
            [(
                "light.toggle".to_owned(),
                "light.toggle – Umschalten".to_owned()
            )]
        );
        let lights = options(
            p.api.as_ref(),
            &[],
            &json!({ "request": "entities", "service": "light.turn_on" }),
        )
        .await
        .unwrap();
        assert_eq!(lights.len(), 2, "empty choice + light.buero: {lights:?}");
        let toggles = options(p.api.as_ref(), &[], &json!({ "request": "toggleable" }))
            .await
            .unwrap();
        assert!(
            toggles.iter().any(|(id, _)| id == "switch.kaffee")
                && !toggles.iter().any(|(id, _)| id == "sensor.temp")
        );
    }

    #[test]
    fn dimmer_math() {
        let light = json!({ "state": "on", "attributes": { "brightness": 128 } });
        let (service, data, opt) = dimmer_call("light.buero", Some(&light), 20.0).unwrap();
        assert_eq!(service, "light.turn_on");
        assert_eq!(data["brightness_step_pct"], 20);
        assert_eq!(opt.unwrap().1, json!(179.0));
        let player = json!({ "state": "playing", "attributes": { "volume_level": 0.95 } });
        let (_, data, _) = dimmer_call("media_player.wz", Some(&player), 10.0).unwrap();
        assert_eq!(data["volume_level"], 1.0, "clamped");
        assert!(dimmer_call("sensor.temp", None, 10.0).is_err());
        assert_eq!(
            level_text("light.x", &json!({ "brightness": 255 }), "on"),
            "100 %"
        );
    }

    #[test]
    fn webhook_list_and_texts() {
        assert_eq!(
            parse_webhooks(" a = b \n\nc"),
            [
                ("b".to_owned(), "a".to_owned()),
                ("c".to_owned(), "c".to_owned())
            ]
        );
        assert_eq!(
            state_text(&json!({ "state": "off", "attributes": {} })),
            "aus"
        );
        assert!(
            Api::new("homeassistant.local", "t").is_err(),
            "needs http(s)://"
        );
    }
}

#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 lights plugin: switches, dims and tints Elgato Key Lights,
//! Philips Hue lights and rooms, and WLED strips on the local network
//! (see `docs/PLUGIN_LIGHTS.md`).

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow, bail, ensure};
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use reqwest::Method;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_millis(100);
/// Visible keys are read every 5 s.
const READ_EVERY_TICKS: u64 = 50;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const PAIR_WINDOW: Duration = Duration::from_secs(30);
const PAIR_INTERVAL: Duration = Duration::from_secs(2);
const ELGATO_PORT: u16 = 9123;
/// Elgato and Hue both express colour temperature in mired (1 000 000 / K).
const ELGATO_MIRED: (u32, u32) = (143, 344);
const HUE_MIRED: (u32, u32) = (153, 500);
const DEFAULT_KELVIN: u32 = 4500;
const KELVIN_STEP: i64 = 100;
const DEFAULT_STEP: f64 = 5.0;
const NOT_PAIRED: &str = "Hue-Bridge nicht gekoppelt (Plugins → Licht → Einstellungen)";
const UNSUPPORTED: &str = "Diese Aktion gibt es für die Lampenart nicht";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Elgato,
    Hue,
    Wled,
}

impl Kind {
    fn from_setting(text: &str) -> anyhow::Result<Self> {
        Ok(match text {
            "elgato" => Self::Elgato,
            "hue" => Self::Hue,
            "wled" => Self::Wled,
            other => bail!("unbekannte Lampenart {other}"),
        })
    }

    /// Device values for 0 % and 100 %.
    fn brightness_range(self) -> (u32, u32) {
        match self {
            Self::Elgato => (3, 100),
            Self::Hue => (1, 254),
            Self::Wled => (0, 255),
        }
    }

    /// Mired range of the colour temperature; None if the device has none.
    fn mired_range(self) -> Option<(u32, u32)> {
        match self {
            Self::Elgato => Some(ELGATO_MIRED),
            Self::Hue => Some(HUE_MIRED),
            Self::Wled => None,
        }
    }

    /// Kelvin limits that match `mired_range`.
    fn kelvin_bounds(self) -> Option<(u32, u32)> {
        self.mired_range()
            .map(|(low, high)| (kelvin_from_mired(high), kelvin_from_mired(low)))
    }
}

/// Last known state of one lamp, room or strip.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Lamp {
    on: bool,
    /// 0 to 100, see `device_to_percent`.
    percent: u32,
    kelvin: Option<u32>,
    /// False until a read succeeded; keys then show "offline".
    online: bool,
}

/// What a key or dial controls. `light` is `light:<id>` or `group:<id>` (Hue only).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Target {
    kind: Kind,
    /// Device address; for Hue the bridge.
    host: String,
    light: String,
}

/// A change to send to a device.
#[derive(Debug)]
enum Write {
    Power(bool),
    Brightness(u32),
    Kelvin(u32),
    Scene { group: String, id: String },
    Preset(u32),
}

/// One HTTP request to a device.
struct Call {
    method: Method,
    url: String,
    body: Value,
}

enum Msg {
    Host(Value),
    /// `None`: the device did not answer.
    Read(Target, Option<Lamp>),
    Done(String),
    Failed(String, String),
    Paired(anyhow::Result<String>),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    http: reqwest::Client,
    contexts: Contexts,
    global: Map<String, Value>,
    /// The first global settings message only loads the stored values.
    loaded: bool,
    lamps: HashMap<Target, Lamp>,
    reading: HashSet<Target>,
    /// Dial changes waiting for the next tick; the latest value per key wins.
    pending: HashMap<String, (Target, Write)>,
    ticks: u64,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>, http: reqwest::Client) -> Self {
        Self {
            host,
            tx,
            http,
            contexts: Contexts::default(),
            global: Map::new(),
            loaded: false,
            lamps: HashMap::new(),
            reading: HashSet::new(),
            pending: HashMap::new(),
            ticks: 0,
        }
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.global.insert("status".into(), json!(status.into()));
        self.host.save_global(&self.global);
    }

    fn on_global(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        let pressed = self.loaded
            && self.global.get("pair").is_some()
            && self.global.get("pair") != previous.get("pair");
        self.loaded = true;
        if pressed {
            self.pair();
        }
    }

    fn pair(&mut self) {
        self.set_status("Bitte den Knopf an der Bridge drücken …");
        let (http, tx) = (self.http.clone(), self.tx.clone());
        let bridge = global_str(&self.global, "hueBridge");
        tokio::spawn(async move {
            tx.send(Msg::Paired(pair_with_bridge(&http, &bridge).await))
                .ok();
        });
    }

    /// Resolves the device an action controls.
    fn target(&self, inst: &Instance) -> anyhow::Result<Target> {
        let kind = Kind::from_setting(&inst.text_or("kind", "elgato"))?;
        let (host, light) = match kind {
            Kind::Hue => {
                hue_user(&self.global)?;
                (global_str(&self.global, "hueBridge"), inst.text("light"))
            }
            Kind::Elgato | Kind::Wled => (inst.text("host"), String::new()),
        };
        check_host(&host)?;
        Ok(Target { kind, host, light })
    }

    /// Reads the lamps of all visible keys that are not read yet.
    fn poll(&mut self) {
        let user = global_str(&self.global, "hueUser");
        let targets: HashSet<Target> = self
            .contexts
            .iter()
            .filter(|(_, inst)| matches!(inst.kind(), "power" | "brightness" | "temperature"))
            .filter_map(|(_, inst)| self.target(inst).ok())
            .filter(|target| !self.reading.contains(target))
            .collect();
        for target in targets {
            self.reading.insert(target.clone());
            let (http, tx, user) = (self.http.clone(), self.tx.clone(), user.clone());
            tokio::spawn(async move {
                let lamp = read(&http, &target, &user).await.ok();
                tx.send(Msg::Read(target, lamp)).ok();
            });
        }
    }

    fn on_read(&mut self, target: Target, lamp: Option<Lamp>) {
        self.reading.remove(&target);
        let entry = self.lamps.entry(target).or_default();
        match lamp {
            Some(fresh) => *entry = fresh,
            None => entry.online = false,
        }
        self.refresh_all();
    }

    /// Every 100 ms: send pending dial values; every 5 s: read visible lamps.
    fn on_tick(&mut self) {
        self.ticks += 1;
        self.flush();
        if self.ticks.is_multiple_of(READ_EVERY_TICKS) {
            self.poll();
        }
    }

    fn flush(&mut self) {
        for (context, (target, write)) in std::mem::take(&mut self.pending) {
            self.send_in_background(&context, target, write, false);
        }
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        if inst.kind() == "scene" {
            return (0, String::new());
        }
        let Ok(target) = self.target(inst) else {
            return (0, String::new());
        };
        let lamp = self.lamps.get(&target).copied().unwrap_or_default();
        if !lamp.online {
            return (0, "offline".into());
        }
        match inst.kind() {
            "power" => (u16::from(lamp.on), String::new()),
            "brightness" => (
                u16::from(lamp.on),
                if lamp.on {
                    format!("{} %", lamp.percent)
                } else {
                    "aus".into()
                },
            ),
            "temperature" => (0, lamp.kelvin.map_or_else(|| "–".into(), kelvin_text)),
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
            "dialRotate" => self.rotate(&context, msg["payload"]["ticks"].as_i64().unwrap_or(0)),
            _ => {}
        }
    }

    fn press(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        let outcome = match inst.kind() {
            "power" | "brightness" => self.toggle(&inst),
            "scene" => self.scene(&inst),
            _ => return,
        };
        match outcome {
            Ok((target, write)) => {
                self.refresh_all();
                self.send_in_background(context, target, write, true);
            }
            Err(err) => {
                self.host.alert(context);
                self.host.log(format!("{err:#}"));
            }
        }
    }

    /// Switches the lamp to the opposite of the last known state.
    fn toggle(&mut self, inst: &Instance) -> anyhow::Result<(Target, Write)> {
        let target = self.target(inst)?;
        let lamp = self.lamps.entry(target.clone()).or_default();
        lamp.on = !lamp.on;
        lamp.online = true;
        let write = Write::Power(lamp.on);
        Ok((target, write))
    }

    fn scene(&self, inst: &Instance) -> anyhow::Result<(Target, Write)> {
        let target = self.target(inst)?;
        let write = match target.kind {
            Kind::Hue => {
                let scene = inst.text("scene");
                let (group, id) = scene.split_once(':').context("Szene wählen")?;
                Write::Scene {
                    group: group.into(),
                    id: id.into(),
                }
            }
            Kind::Wled => {
                let preset = inst.number("preset", 1.0);
                ensure!((1.0..=250.0).contains(&preset), "Preset 1 bis 250 wählen");
                Write::Preset(preset as u32)
            }
            Kind::Elgato => bail!("Elgato-Lampen haben keine Szenen"),
        };
        Ok((target, write))
    }

    /// Applies the dial step at once; the device gets the value on the next tick.
    fn rotate(&mut self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        if ticks == 0 || !matches!(inst.kind(), "brightness" | "temperature") {
            return;
        }
        match self.dial_write(&inst, ticks) {
            Ok((target, write)) => {
                let lamp = self.lamps.entry(target.clone()).or_default();
                lamp.online = true;
                match &write {
                    Write::Brightness(percent) => {
                        lamp.on = true;
                        lamp.percent = *percent;
                    }
                    Write::Kelvin(kelvin) => lamp.kelvin = Some(*kelvin),
                    _ => {}
                }
                self.pending.insert(context.to_owned(), (target, write));
                self.refresh_all();
            }
            Err(err) => {
                self.host.alert(context);
                self.host.log(format!("{err:#}"));
            }
        }
    }

    fn dial_write(&self, inst: &Instance, ticks: i64) -> anyhow::Result<(Target, Write)> {
        let target = self.target(inst)?;
        let lamp = self.lamps.get(&target).copied().unwrap_or_default();
        let write = if inst.kind() == "brightness" {
            Write::Brightness(step_percent(
                lamp.percent,
                ticks,
                inst.number("step", DEFAULT_STEP),
            ))
        } else {
            let (low, high) = target
                .kind
                .kelvin_bounds()
                .context("WLED unterstützt keine Farbtemperatur")?;
            let current = i64::from(lamp.kelvin.unwrap_or(DEFAULT_KELVIN));
            let kelvin = (current + ticks * KELVIN_STEP).clamp(i64::from(low), i64::from(high));
            Write::Kelvin(kelvin as u32)
        };
        Ok((target, write))
    }

    /// Sends a write in its own task. `flash` shows the result on the key.
    fn send_in_background(&self, context: &str, target: Target, write: Write, flash: bool) {
        let (http, tx, context) = (self.http.clone(), self.tx.clone(), context.to_owned());
        let user = global_str(&self.global, "hueUser");
        tokio::spawn(async move {
            let msg = match send(&http, &target, &user, &write).await {
                Ok(()) if flash => Msg::Done(context),
                Ok(()) => return,
                Err(err) => Msg::Failed(context, format!("{err:#}")),
            };
            tx.send(msg).ok();
        });
    }

    fn on_request(&self, req: Value) {
        let (host, http) = (self.host.clone(), self.http.clone());
        let bridge = global_str(&self.global, "hueBridge");
        let user = global_str(&self.global, "hueUser");
        tokio::spawn(async move {
            let result = options(&http, &bridge, &user, &req).await;
            host.reply_options(&req, result);
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Read(target, lamp) => self.on_read(target, lamp),
            Msg::Done(context) => self.host.ok(&context),
            Msg::Failed(context, error) => {
                self.host.alert(&context);
                self.host.log(error);
            }
            Msg::Paired(Ok(user)) => {
                self.global.insert("hueUser".into(), json!(user));
                self.set_status("Mit Hue-Bridge gekoppelt");
            }
            Msg::Paired(Err(err)) => self.set_status(format!("Kopplung fehlgeschlagen: {err:#}")),
        }
    }
}

/// Device address goes into a URL, so only host names and IPv4 addresses pass.
fn check_host(host: &str) -> anyhow::Result<()> {
    ensure!(!host.is_empty(), "IP-Adresse der Lampe eintragen");
    ensure!(
        host.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'),
        "Adresse ungültig: nur IP-Adresse oder Hostname"
    );
    Ok(())
}

/// Hue ids and usernames are alphanumeric; anything else must not reach a URL.
fn valid_id(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn hue_user(global: &Map<String, Value>) -> anyhow::Result<String> {
    let user = global_str(global, "hueUser");
    ensure!(valid_id(&user), NOT_PAIRED);
    Ok(user)
}

fn hue_url(bridge: &str, user: &str, path: &str) -> anyhow::Result<String> {
    check_host(bridge)?;
    ensure!(valid_id(user), NOT_PAIRED);
    Ok(format!("http://{bridge}/api/{user}/{path}"))
}

/// `light:3` → (`lights`, `3`); `group:1` → (`groups`, `1`).
fn hue_resource(light: &str) -> anyhow::Result<(&'static str, &str)> {
    let Some((kind, id)) = light.split_once(':') else {
        bail!("Lampe oder Raum wählen");
    };
    ensure!(valid_id(id), "Lampe oder Raum wählen");
    match kind {
        "light" => Ok(("lights", id)),
        "group" => Ok(("groups", id)),
        _ => bail!("Lampe oder Raum wählen"),
    }
}

fn elgato_url(host: &str) -> String {
    format!("http://{host}:{ELGATO_PORT}/elgato/lights")
}

fn state_url(target: &Target, user: &str) -> anyhow::Result<String> {
    Ok(match target.kind {
        Kind::Elgato => elgato_url(&target.host),
        Kind::Hue => {
            let (resource, id) = hue_resource(&target.light)?;
            hue_url(&target.host, user, &format!("{resource}/{id}"))?
        }
        Kind::Wled => format!("http://{}/json/state", target.host),
    })
}

fn command(target: &Target, user: &str, write: &Write) -> anyhow::Result<Call> {
    match target.kind {
        Kind::Elgato => elgato_command(target, write),
        Kind::Hue => hue_command(target, user, write),
        Kind::Wled => wled_command(target, write),
    }
}

fn elgato_command(target: &Target, write: &Write) -> anyhow::Result<Call> {
    let light = match write {
        Write::Power(on) => json!({ "on": u8::from(*on) }),
        Write::Brightness(percent) => json!({
            "on": 1,
            "brightness": percent_to_device(Kind::Elgato, *percent),
        }),
        Write::Kelvin(kelvin) => json!({ "temperature": clamp_mired(ELGATO_MIRED, *kelvin) }),
        Write::Scene { .. } | Write::Preset(_) => bail!(UNSUPPORTED),
    };
    Ok(Call {
        method: Method::PUT,
        url: elgato_url(&target.host),
        body: json!({ "lights": [light] }),
    })
}

fn hue_command(target: &Target, user: &str, write: &Write) -> anyhow::Result<Call> {
    if let Write::Scene { group, id } = write {
        ensure!(valid_id(group) && valid_id(id), "Szene ungültig");
        return Ok(Call {
            method: Method::PUT,
            url: hue_url(&target.host, user, &format!("groups/{group}/action"))?,
            body: json!({ "scene": id }),
        });
    }
    let (resource, id) = hue_resource(&target.light)?;
    // Rooms take their state at `action`, single lights at `state`.
    let path = if resource == "groups" {
        format!("groups/{id}/action")
    } else {
        format!("lights/{id}/state")
    };
    let body = match write {
        Write::Power(on) => json!({ "on": *on }),
        Write::Brightness(percent) => json!({
            "on": true,
            "bri": percent_to_device(Kind::Hue, *percent),
        }),
        Write::Kelvin(kelvin) => json!({ "ct": clamp_mired(HUE_MIRED, *kelvin) }),
        Write::Scene { .. } | Write::Preset(_) => bail!(UNSUPPORTED),
    };
    Ok(Call {
        method: Method::PUT,
        url: hue_url(&target.host, user, &path)?,
        body,
    })
}

fn wled_command(target: &Target, write: &Write) -> anyhow::Result<Call> {
    let body = match write {
        Write::Power(on) => json!({ "on": *on }),
        Write::Brightness(percent) => json!({
            "on": true,
            "bri": percent_to_device(Kind::Wled, *percent),
        }),
        Write::Preset(preset) => json!({ "ps": preset }),
        Write::Kelvin(_) => bail!("WLED unterstützt keine Farbtemperatur"),
        Write::Scene { .. } => bail!(UNSUPPORTED),
    };
    Ok(Call {
        method: Method::POST,
        url: format!("http://{}/json/state", target.host),
        body,
    })
}

fn num(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

/// Lamp state from the answer of `state_url`.
fn parse_lamp(target: &Target, body: &Value) -> Option<Lamp> {
    let (on, percent, kelvin) = match target.kind {
        Kind::Elgato => {
            let light = &body["lights"][0];
            (
                light["on"].as_u64()? == 1,
                device_to_percent(Kind::Elgato, num(&light["brightness"])?),
                num(&light["temperature"]).map(kelvin_from_mired),
            )
        }
        Kind::Hue => {
            // A room reports `any_on` and its values under `action`.
            let (on, values) = if target.light.starts_with("group:") {
                (&body["state"]["any_on"], &body["action"])
            } else {
                (&body["state"]["on"], &body["state"])
            };
            (
                on.as_bool()?,
                device_to_percent(Kind::Hue, num(&values["bri"])?),
                num(&values["ct"]).map(kelvin_from_mired),
            )
        }
        Kind::Wled => (
            body["on"].as_bool()?,
            device_to_percent(Kind::Wled, num(&body["bri"])?),
            None,
        ),
    };
    Some(Lamp {
        on,
        percent,
        kelvin,
        online: true,
    })
}

/// Hue reports failures inside a 200 answer: `[{"error":{"description":…}}]`.
fn device_error(body: &Value) -> Option<String> {
    body.as_array()?
        .iter()
        .find_map(|item| item["error"]["description"].as_str())
        .map(str::to_owned)
}

fn unreachable(host: &str, err: reqwest::Error) -> anyhow::Error {
    // Hue URLs contain the username; keep them out of messages and logs.
    anyhow!("Gerät {host} nicht erreichbar: {}", err.without_url())
}

async fn get(http: &reqwest::Client, host: &str, url: &str) -> anyhow::Result<Value> {
    let res = http
        .get(url)
        .send()
        .await
        .map_err(|err| unreachable(host, err))?;
    ensure!(
        res.status().is_success(),
        "Gerät antwortet mit HTTP {}",
        res.status().as_u16()
    );
    res.json().await.context("Antwort nicht lesbar")
}

async fn read(http: &reqwest::Client, target: &Target, user: &str) -> anyhow::Result<Lamp> {
    let body = get(http, &target.host, &state_url(target, user)?).await?;
    parse_lamp(target, &body).context("unerwartete Antwort")
}

async fn send(
    http: &reqwest::Client,
    target: &Target,
    user: &str,
    write: &Write,
) -> anyhow::Result<()> {
    let call = command(target, user, write)?;
    let res = http
        .request(call.method, &call.url)
        .json(&call.body)
        .send()
        .await
        .map_err(|err| unreachable(&target.host, err))?;
    ensure!(
        res.status().is_success(),
        "Gerät antwortet mit HTTP {}",
        res.status().as_u16()
    );
    let body: Value = res.json().await.unwrap_or_default();
    match device_error(&body) {
        Some(message) => bail!("{message}"),
        None => Ok(()),
    }
}

#[derive(Debug, PartialEq)]
enum Pairing {
    Done(String),
    Waiting,
    Failed(String),
}

/// Answer of `POST /api`; error type 101 means the bridge button was not pressed yet.
fn classify_pairing(body: &Value) -> Pairing {
    let first = &body[0];
    if let Some(user) = first["success"]["username"].as_str() {
        return Pairing::Done(user.to_owned());
    }
    match first["error"]["type"].as_u64() {
        Some(101) => Pairing::Waiting,
        _ => Pairing::Failed(
            first["error"]["description"]
                .as_str()
                .unwrap_or("unbekannte Antwort der Bridge")
                .to_owned(),
        ),
    }
}

async fn pair_with_bridge(http: &reqwest::Client, bridge: &str) -> anyhow::Result<String> {
    check_host(bridge)?;
    let deadline = Instant::now() + PAIR_WINDOW;
    loop {
        let res = http
            .post(format!("http://{bridge}/api"))
            .json(&json!({ "devicetype": "opendeckn3#pc" }))
            .send()
            .await
            .map_err(|err| unreachable(bridge, err))?;
        let body: Value = res
            .json()
            .await
            .context("Antwort der Bridge nicht lesbar")?;
        match classify_pairing(&body) {
            Pairing::Done(user) => return Ok(user),
            Pairing::Failed(message) => bail!("{message}"),
            Pairing::Waiting if Instant::now() >= deadline => {
                bail!("Knopf an der Bridge nicht innerhalb von 30 s gedrückt")
            }
            Pairing::Waiting => tokio::time::sleep(PAIR_INTERVAL).await,
        }
    }
}

/// Dropdown entries for the Hue bridge: lamps, then rooms and groups.
fn hue_lamp_options(lights: &Value, groups: &Value) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = Vec::new();
    for (id, light) in lights.as_object().into_iter().flatten() {
        let name = light["name"].as_str().unwrap_or(id.as_str());
        list.push((format!("light:{id}"), format!("{name} (Lampe)")));
    }
    for (id, group) in groups.as_object().into_iter().flatten() {
        let name = group["name"].as_str().unwrap_or(id.as_str());
        let kind = if group["type"] == "Room" {
            "Raum"
        } else {
            "Gruppe"
        };
        list.push((format!("group:{id}"), format!("{name} ({kind})")));
    }
    list.sort_by_key(|(_, label)| label.to_lowercase());
    list
}

/// Scene value is `group:id`; scenes without a group (light scenes) use group 0.
fn hue_scene_options(scenes: &Value) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = scenes
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, scene)| {
            let group = scene["group"].as_str().unwrap_or("0");
            let name = scene["name"].as_str().unwrap_or(id.as_str());
            (format!("{group}:{id}"), name.to_owned())
        })
        .collect();
    list.sort_by_key(|(_, label)| label.to_lowercase());
    list
}

async fn options(
    http: &reqwest::Client,
    bridge: &str,
    user: &str,
    req: &Value,
) -> anyhow::Result<Vec<(String, String)>> {
    match req["request"].as_str().unwrap_or_default() {
        "hueLights" => {
            let lights = get(http, bridge, &hue_url(bridge, user, "lights")?).await?;
            let groups = get(http, bridge, &hue_url(bridge, user, "groups")?).await?;
            Ok(hue_lamp_options(&lights, &groups))
        }
        "hueScenes" => {
            let scenes = get(http, bridge, &hue_url(bridge, user, "scenes")?).await?;
            Ok(hue_scene_options(&scenes))
        }
        other => bail!("unbekannte Anfrage {other}"),
    }
}

/// Device value for a 0–100 % brightness, spread over the device's range.
fn percent_to_device(kind: Kind, percent: u32) -> u32 {
    let (min, max) = kind.brightness_range();
    min + (percent.min(100) * (max - min) + 50) / 100
}

fn device_to_percent(kind: Kind, value: u32) -> u32 {
    let (min, max) = kind.brightness_range();
    ((value.clamp(min, max) - min) * 100 + (max - min) / 2) / (max - min)
}

fn kelvin_from_mired(mired: u32) -> u32 {
    (1_000_000 + mired / 2) / mired.max(1)
}

fn mired_from_kelvin(kelvin: u32) -> u32 {
    (1_000_000 + kelvin / 2) / kelvin.max(1)
}

/// Device value for a Kelvin target, kept inside the device's range.
fn clamp_mired(range: (u32, u32), kelvin: u32) -> u32 {
    mired_from_kelvin(kelvin).clamp(range.0, range.1)
}

/// Rounded to 100 K, e.g. `4500 K`.
fn kelvin_text(kelvin: u32) -> String {
    format!("{} K", (kelvin + 50) / 100 * 100)
}

fn step_percent(current: u32, ticks: i64, step: f64) -> u32 {
    (f64::from(current) + ticks as f64 * step)
        .round()
        .clamp(0.0, 100.0) as u32
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    let mut plugin = Plugin::new(host, tx, http);
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.handle(Msg::Host(msg)),
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

    fn target(kind: Kind, host: &str, light: &str) -> Target {
        Target {
            kind,
            host: host.into(),
            light: light.into(),
        }
    }

    #[test]
    fn brightness_maps_onto_the_device_range() {
        assert_eq!(percent_to_device(Kind::Elgato, 0), 3);
        assert_eq!(percent_to_device(Kind::Elgato, 100), 100);
        assert_eq!(percent_to_device(Kind::Hue, 100), 254);
        assert_eq!(percent_to_device(Kind::Hue, 50), 128);
        assert_eq!(percent_to_device(Kind::Wled, 50), 128);
        assert_eq!(percent_to_device(Kind::Wled, 150), 255, "clamped");
        assert_eq!(device_to_percent(Kind::Hue, 1), 0);
        assert_eq!(device_to_percent(Kind::Hue, 254), 100);
        assert_eq!(device_to_percent(Kind::Wled, 999), 100, "clamped");
        for kind in [Kind::Elgato, Kind::Hue, Kind::Wled] {
            for percent in 0..=100 {
                let back = device_to_percent(kind, percent_to_device(kind, percent));
                assert!(back.abs_diff(percent) <= 1, "{kind:?} {percent} -> {back}");
            }
        }
    }

    #[test]
    fn colour_temperature_converts_between_kelvin_and_mired() {
        assert_eq!(kelvin_from_mired(143), 6993);
        assert_eq!(mired_from_kelvin(7000), 143);
        assert_eq!(
            clamp_mired(ELGATO_MIRED, 2900),
            344,
            "clamped to the device"
        );
        assert_eq!(clamp_mired(HUE_MIRED, 9000), 153);
        assert_eq!(kelvin_text(4505), "4500 K");
        assert_eq!(Kind::Wled.kelvin_bounds(), None);
        assert_eq!(Kind::Hue.kelvin_bounds(), Some((2000, 6536)));
    }

    #[test]
    fn dial_steps_are_clamped() {
        assert_eq!(step_percent(98, 2, 5.0), 100);
        assert_eq!(step_percent(3, -1, 5.0), 0);
        assert_eq!(step_percent(50, 1, 2.5), 53);
    }

    #[test]
    fn lamp_states_are_read_from_each_api() {
        let elgato = parse_lamp(
            &target(Kind::Elgato, "10.0.0.5", ""),
            &json!({ "lights": [{ "on": 1, "brightness": 50, "temperature": 200 }] }),
        )
        .unwrap();
        assert!(elgato.on && elgato.online);
        assert_eq!(elgato.percent, device_to_percent(Kind::Elgato, 50));
        assert_eq!(elgato.kelvin, Some(5000));

        let room = parse_lamp(
            &target(Kind::Hue, "10.0.0.20", "group:1"),
            &json!({ "state": { "any_on": true, "all_on": false }, "action": { "on": true, "bri": 254, "ct": 153 } }),
        )
        .unwrap();
        assert!(room.on);
        assert_eq!(room.percent, 100);
        assert_eq!(room.kelvin, Some(6536));

        let light = parse_lamp(
            &target(Kind::Hue, "10.0.0.20", "light:3"),
            &json!({ "state": { "on": false, "bri": 1, "ct": 500 } }),
        )
        .unwrap();
        assert!(!light.on);
        assert_eq!(light.percent, 0);
        assert_eq!(light.kelvin, Some(2000));

        let wled = parse_lamp(
            &target(Kind::Wled, "10.0.0.7", ""),
            &json!({ "on": true, "bri": 255 }),
        )
        .unwrap();
        assert_eq!((wled.percent, wled.kelvin), (100, None));
    }

    #[test]
    fn commands_match_each_device_api() {
        let elgato = command(
            &target(Kind::Elgato, "10.0.0.5", ""),
            "",
            &Write::Power(false),
        )
        .unwrap();
        assert_eq!(elgato.url, "http://10.0.0.5:9123/elgato/lights");
        assert_eq!(elgato.body, json!({ "lights": [{ "on": 0 }] }));

        let room = command(
            &target(Kind::Hue, "10.0.0.20", "group:1"),
            "abc123",
            &Write::Brightness(100),
        )
        .unwrap();
        assert_eq!(room.url, "http://10.0.0.20/api/abc123/groups/1/action");
        assert_eq!(room.body, json!({ "on": true, "bri": 254 }));

        let light = command(
            &target(Kind::Hue, "10.0.0.20", "light:3"),
            "abc123",
            &Write::Kelvin(4000),
        )
        .unwrap();
        assert_eq!(light.url, "http://10.0.0.20/api/abc123/lights/3/state");
        assert_eq!(light.body, json!({ "ct": clamp_mired(HUE_MIRED, 4000) }));

        let scene = command(
            &target(Kind::Hue, "10.0.0.20", ""),
            "abc123",
            &Write::Scene {
                group: "2".into(),
                id: "xyz".into(),
            },
        )
        .unwrap();
        assert_eq!(scene.url, "http://10.0.0.20/api/abc123/groups/2/action");
        assert_eq!(scene.body, json!({ "scene": "xyz" }));

        let preset = command(&target(Kind::Wled, "10.0.0.7", ""), "", &Write::Preset(3)).unwrap();
        assert_eq!(preset.body, json!({ "ps": 3 }));

        assert!(
            command(
                &target(Kind::Wled, "10.0.0.7", ""),
                "",
                &Write::Kelvin(4000)
            )
            .is_err()
        );
        assert!(
            command(
                &target(Kind::Hue, "10.0.0.20", "light:1/../x"),
                "abc123",
                &Write::Power(true),
            )
            .is_err(),
            "ids are checked before they reach a URL"
        );
        assert!(
            command(
                &target(Kind::Hue, "10.0.0.20", "light:1"),
                "",
                &Write::Power(true)
            )
            .is_err(),
            "no user, no request"
        );
    }

    #[test]
    fn pairing_waits_for_the_bridge_button() {
        let waiting = json!([{ "error": { "type": 101, "address": "/", "description": "link button not pressed" } }]);
        assert_eq!(classify_pairing(&waiting), Pairing::Waiting);
        let done = json!([{ "success": { "username": "abc123" } }]);
        assert_eq!(classify_pairing(&done), Pairing::Done("abc123".into()));
        let other = json!([{ "error": { "type": 3, "description": "not available" } }]);
        assert_eq!(
            classify_pairing(&other),
            Pairing::Failed("not available".into())
        );
    }

    #[test]
    fn hue_errors_inside_a_successful_answer_are_reported() {
        assert_eq!(
            device_error(&json!([{ "error": { "description": "resource not available" } }])),
            Some("resource not available".into())
        );
        assert_eq!(device_error(&json!([{ "success": { "on": true } }])), None);
    }

    #[test]
    fn dropdowns_list_lamps_rooms_and_scenes() {
        let lamps = hue_lamp_options(
            &json!({ "3": { "name": "Stehlampe" } }),
            &json!({ "1": { "name": "Wohnzimmer", "type": "Room" } }),
        );
        assert_eq!(
            lamps,
            [
                ("light:3".to_owned(), "Stehlampe (Lampe)".to_owned()),
                ("group:1".to_owned(), "Wohnzimmer (Raum)".to_owned()),
            ]
        );
        let scenes = hue_scene_options(&json!({
            "abc": { "name": "Abend", "group": "1" },
            "def": { "name": "Hell" },
        }));
        assert_eq!(
            scenes,
            [
                ("1:abc".to_owned(), "Abend".to_owned()),
                ("0:def".to_owned(), "Hell".to_owned()),
            ]
        );
    }

    #[test]
    fn dial_keeps_only_the_latest_value_until_the_next_tick() {
        let (host, _rx) = Host::channel("de.opendeckn3.lights");
        let (tx, _msgs) = mpsc::unbounded_channel();
        let mut plugin = Plugin::new(host, tx, reqwest::Client::new());
        plugin.contexts.apply(&json!({
            "event": "willAppear",
            "context": "d",
            "action": "de.opendeckn3.lights.brightness",
            "payload": { "controller": "Encoder", "settings": { "kind": "wled", "host": "10.0.0.7" } },
        }));
        plugin.on_host(json!({ "event": "dialRotate", "context": "d", "payload": { "ticks": 1 } }));
        plugin.on_host(json!({ "event": "dialRotate", "context": "d", "payload": { "ticks": 2 } }));
        assert_eq!(plugin.pending.len(), 1);
        let (target, write) = &plugin.pending["d"];
        assert_eq!(target.host, "10.0.0.7");
        assert!(
            matches!(write, Write::Brightness(15)),
            "5 % then 10 % more: {write:?}"
        );
    }
}

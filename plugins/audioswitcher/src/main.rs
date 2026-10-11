#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 AudioSwitcher plugin: activates and toggles the audio profiles
//! of AudioSwitcher (https://github.com/Diddlik/AudioSwitcher) over its local
//! control pipe (see `docs/PLUGIN_AUDIOSWITCHER.md`).

use std::time::Duration;

use anyhow::{Context, bail};
use n3_plugin_sdk::{Contexts, Host, Instance};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
};

const TICK: Duration = Duration::from_secs(2);
// The control pipe exists only on Windows; elsewhere every request fails early.
#[cfg_attr(not(windows), allow(dead_code))]
const NOT_RUNNING: &str = "AudioSwitcher läuft nicht oder ist zu alt für die Fernsteuerung";

/// One request/response round trip: a JSON line out, a JSON line back.
#[cfg_attr(not(windows), allow(dead_code))]
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    request: &Value,
) -> anyhow::Result<Value> {
    let mut io = BufReader::new(stream);
    io.get_mut()
        .write_all(format!("{request}\n").as_bytes())
        .await?;
    let mut line = String::new();
    anyhow::ensure!(
        io.read_line(&mut line).await? > 0,
        "AudioSwitcher hat die Verbindung getrennt"
    );
    serde_json::from_str(&line).context("unverständliche Antwort von AudioSwitcher")
}

#[cfg(windows)]
async fn request(body: Value) -> anyhow::Result<Value> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const PIPE: &str = r"\\.\pipe\AudioSwitcher.Control";
    const ERROR_PIPE_BUSY: i32 = 231;
    const TIMEOUT: Duration = Duration::from_secs(5);
    tokio::time::timeout(TIMEOUT, async {
        let pipe = loop {
            match ClientOptions::new().open(PIPE) {
                Ok(pipe) => break pipe,
                // All server instances are taken for a moment; a new one follows.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(_) => bail!(NOT_RUNNING),
            }
        };
        exchange(pipe, &body).await
    })
    .await
    .unwrap_or_else(|_| bail!("AudioSwitcher antwortet nicht"))
}

#[cfg(not(windows))]
async fn request(_body: Value) -> anyhow::Result<Value> {
    bail!("AudioSwitcher gibt es nur für Windows")
}

#[derive(Clone, Debug, PartialEq)]
struct Profile {
    id: String,
    name: String,
    available: bool,
}

enum Msg {
    Host(Value),
    /// Answer to a request; `context` is the key that triggered it (none for polls).
    Reply(Option<String>, anyhow::Result<Value>),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    profiles: Vec<Profile>,
    active: Option<String>,
    online: Option<bool>,
    polling: bool,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            global: Map::new(),
            profiles: Vec::new(),
            active: None,
            online: None,
            polling: false,
        }
    }

    fn send(&self, context: Option<&str>, body: Value) {
        let (tx, context) = (self.tx.clone(), context.map(str::to_owned));
        tokio::spawn(async move {
            tx.send(Msg::Reply(context, request(body).await)).ok();
        });
    }

    fn poll(&mut self) {
        if self.polling || self.contexts.ids().is_empty() && self.online.is_some() {
            return;
        }
        self.polling = true;
        self.send(None, json!({ "command": "state" }));
    }

    fn set_status(&mut self, status: String) {
        if self.global.get("status").and_then(Value::as_str) != Some(status.as_str()) {
            self.global.insert("status".into(), json!(status));
            self.host.save_global(&self.global);
        }
    }

    fn on_reply(&mut self, context: Option<String>, result: anyhow::Result<Value>) {
        if context.is_none() {
            self.polling = false;
        }
        let reply = match result {
            Ok(reply) => reply,
            Err(err) => {
                self.online = Some(false);
                self.set_status(format!("{err:#}"));
                if let Some(context) = &context {
                    self.host.alert(context);
                }
                self.refresh_all();
                return;
            }
        };
        self.online = Some(true);
        self.profiles = reply["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some(Profile {
                    id: p["id"].as_str()?.to_owned(),
                    name: p["name"].as_str().unwrap_or_default().to_owned(),
                    available: p["available"].as_bool().unwrap_or(true),
                })
            })
            .collect();
        self.active = reply["activeProfileId"].as_str().map(str::to_owned);
        let count = self.profiles.len();
        self.set_status(format!(
            "Verbunden mit AudioSwitcher – {count} {}",
            if count == 1 { "Profil" } else { "Profile" }
        ));
        if reply["ok"] == false {
            let error = reply["error"].as_str().unwrap_or("Fehler");
            self.host.log(format!("AudioSwitcher: {error}"));
            if let Some(context) = &context {
                self.host.alert(context);
            }
        }
        self.refresh_all();
    }

    fn active_name(&self) -> Option<&str> {
        let active = self.active.as_deref()?;
        self.profiles
            .iter()
            .find(|p| p.id == active)
            .map(|p| p.name.as_str())
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        if self.online == Some(false) {
            return (0, "offline".into());
        }
        match inst.kind() {
            "profile" => {
                let id = inst.text("profile");
                let Some(profile) = self.profiles.iter().find(|p| p.id == id) else {
                    let missing = self.online.is_some() && !id.is_empty();
                    return (0, if missing { "fehlt" } else { "" }.into());
                };
                let active = u16::from(self.active.as_deref() == Some(id.as_str()));
                let title = if inst.flag("showName", true) {
                    profile.name.clone()
                } else {
                    String::new()
                };
                (active, title)
            }
            "toggle" => (0, self.active_name().unwrap_or("–").to_owned()),
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

    fn press(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        match inst.kind() {
            "profile" => {
                let id = inst.text("profile");
                if id.is_empty() {
                    self.host.alert(context);
                    return;
                }
                self.send(
                    Some(context),
                    json!({ "command": "activate", "profileId": id }),
                );
            }
            "toggle" => self.send(Some(context), json!({ "command": "toggle" })),
            _ => {}
        }
    }

    /// The dial steps through the available profiles in AudioSwitcher's order.
    fn rotate(&mut self, context: &str, ticks: i64) {
        if self.contexts.get(context).map(Instance::kind) != Some("toggle") {
            return;
        }
        if let Some(id) = step(&self.profiles, self.active.as_deref(), ticks) {
            self.send(
                Some(context),
                json!({ "command": "activate", "profileId": id }),
            );
        }
    }

    fn on_request(&self, req: Value) {
        let host = self.host.clone();
        tokio::spawn(async move {
            let options = match req["request"].as_str() {
                Some("profiles") => request(json!({ "command": "state" })).await.map(|reply| {
                    reply["profiles"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| {
                            Some((p["id"].as_str()?.to_owned(), p["name"].as_str()?.to_owned()))
                        })
                        .collect()
                }),
                other => Err(anyhow::anyhow!("unbekannte Anfrage {other:?}")),
            };
            host.reply_options(&req, options);
        });
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.refresh(&context);
            self.poll();
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => {
                self.global = msg["payload"]["settings"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                self.poll();
            }
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                self.rotate(&context, ticks);
            }
            _ => {}
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Reply(context, result) => self.on_reply(context, result),
        }
    }
}

/// Profile `ticks` steps away from the active one among available profiles.
fn step(profiles: &[Profile], active: Option<&str>, ticks: i64) -> Option<String> {
    let available: Vec<&Profile> = profiles.iter().filter(|p| p.available).collect();
    if available.is_empty() || ticks == 0 {
        return None;
    }
    let len = available.len() as i64;
    let next = match available.iter().position(|p| Some(p.id.as_str()) == active) {
        Some(current) => (current as i64 + ticks).rem_euclid(len),
        // Nothing active: the first step lands on the first or last profile.
        None if ticks > 0 => ticks - 1,
        None => ticks,
    }
    .rem_euclid(len);
    Some(available[next as usize].id.clone())
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

    fn profiles() -> Vec<Profile> {
        ["a", "b", "c"]
            .into_iter()
            .map(|id| Profile {
                id: id.into(),
                name: id.to_uppercase(),
                available: id != "b",
            })
            .collect()
    }

    fn state() -> Value {
        json!({ "ok": true, "error": null, "activeProfileId": "c",
            "profiles": [{ "id": "a", "name": "Lautsprecher", "available": true },
                         { "id": "c", "name": "Headset", "available": true }] })
    }

    #[test]
    fn dial_skips_unavailable_profiles_and_wraps() {
        let p = profiles();
        assert_eq!(step(&p, Some("a"), 1).as_deref(), Some("c"));
        assert_eq!(step(&p, Some("c"), 1).as_deref(), Some("a"));
        assert_eq!(step(&p, Some("a"), -1).as_deref(), Some("c"));
        assert_eq!(step(&p, None, 1).as_deref(), Some("a"));
        assert_eq!(step(&p, None, -1).as_deref(), Some("c"));
        assert_eq!(step(&p, Some("a"), 0), None);
    }

    #[tokio::test]
    async fn exchange_writes_one_line_and_reads_one() {
        let (client, server) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut io = BufReader::new(server);
            let mut line = String::new();
            io.read_line(&mut line).await.unwrap();
            io.get_mut()
                .write_all(format!("{}\n", state()).as_bytes())
                .await
                .unwrap();
            line
        });
        let reply = exchange(client, &json!({ "command": "toggle" }))
            .await
            .unwrap();
        assert_eq!(reply["activeProfileId"], "c");
        assert_eq!(server.await.unwrap(), "{\"command\":\"toggle\"}\n");
    }

    #[tokio::test]
    async fn keys_follow_the_active_profile() {
        let (host, mut host_rx) = Host::channel("de.opendeckn3.audioswitcher");
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut p = Plugin::new(host, tx);
        p.online = Some(true);
        for (context, kind, settings) in [
            ("a", "profile", json!({ "profile": "a" })),
            ("c", "profile", json!({ "profile": "c", "showName": "no" })),
            ("t", "toggle", json!({})),
        ] {
            p.on_host(json!({ "event": "willAppear", "context": context,
                "action": format!("de.opendeckn3.audioswitcher.{kind}"),
                "payload": { "controller": "Keypad", "settings": settings } }));
        }
        std::iter::from_fn(|| host_rx.try_recv().ok()).for_each(drop);
        p.on_reply(None, Ok(state()));
        let sent: Vec<Value> = std::iter::from_fn(|| host_rx.try_recv().ok()).collect();
        let title = |c: &str| {
            sent.iter()
                .find(|m| m["event"] == "setTitle" && m["context"] == c)
                .map(|m| m["payload"]["title"].clone())
        };
        let state = |c: &str| {
            sent.iter()
                .find(|m| m["event"] == "setState" && m["context"] == c)
                .map(|m| m["payload"]["state"].clone())
        };
        assert_eq!(title("a"), Some(json!("Lautsprecher")));
        assert_ne!(state("a"), Some(json!(1)), "only changes are sent");
        assert_eq!(state("c"), Some(json!(1)));
        assert_eq!(title("c"), Some(json!("")));
        assert_eq!(title("t"), Some(json!("Headset")));
        assert!(sent.iter().any(|m| m["event"] == "setGlobalSettings"
            && m["payload"]["status"] == "Verbunden mit AudioSwitcher – 2 Profile"));

        p.on_reply(Some("a".into()), Err(anyhow::anyhow!(NOT_RUNNING)));
        let sent: Vec<Value> = std::iter::from_fn(|| host_rx.try_recv().ok()).collect();
        assert!(
            sent.iter()
                .any(|m| m["event"] == "showAlert" && m["context"] == "a")
        );
        assert!(
            sent.iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == "offline")
        );
    }
}

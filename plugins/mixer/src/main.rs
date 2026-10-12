#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 mixer plugin: changes the volume and mute state of single
//! applications with a dial or a key (Windows: WASAPI sessions, Linux:
//! PulseAudio/PipeWire via `pactl`; see `docs/PLUGIN_MIXER.md`).

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use crate::{Change, Snapshot};

    pub async fn snapshot() -> anyhow::Result<Snapshot> {
        anyhow::bail!("nicht unterstützt")
    }

    pub async fn change(_: String, _: Change) -> anyhow::Result<()> {
        anyhow::bail!("nicht unterstützt")
    }
}

use std::time::Duration;

use anyhow::Context;
use n3_plugin_sdk::{Contexts, Host, Instance};
use serde_json::Value;
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_secs(1);
const DEFAULT_STEP: f64 = 2.0;
const ERROR_TITLE: &str = "–";
const MUTED_TITLE: &str = "stumm";

#[derive(Clone, Copy)]
pub enum Change {
    /// Adds a fraction of full volume to every session of the app.
    VolumeStep(f32),
    /// Mutes all sessions of the app, or unmutes them if the first one is muted.
    ToggleMute,
}

/// One audio session of the default output device.
#[derive(Clone)]
pub struct Session {
    pub app: String,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub muted: bool,
}

#[derive(Default)]
pub struct Snapshot {
    pub sessions: Vec<Session>,
    /// Process name of the window in the foreground, if known.
    pub foreground: Option<String>,
}

enum Msg {
    Host(Value),
    Snapshot(anyhow::Result<Snapshot>),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    busy: bool,
    failing: bool,
    snapshot: Snapshot,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            busy: false,
            failing: false,
            snapshot: Snapshot::default(),
        }
    }

    fn on_host(&mut self, msg: Value) {
        if self.contexts.apply(&msg).is_some() {
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => self.rotate(&context, msg["payload"]["ticks"].as_i64().unwrap_or(0)),
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            _ => {}
        }
    }

    /// The app a key or dial controls: the setting for `app`, the foreground app for `focused`.
    fn target(&self, inst: &Instance) -> Option<String> {
        match inst.kind() {
            "app" => Some(inst.text("app")).filter(|app| !app.is_empty()),
            "focused" => self.snapshot.foreground.clone(),
            _ => None,
        }
    }

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        if let Some(app) = self.target(inst) {
            self.change(context, app, Change::ToggleMute);
        }
    }

    fn rotate(&self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        if ticks == 0 {
            return;
        }
        let step = inst.number("step", DEFAULT_STEP) as f32;
        if let Some(app) = self.target(inst) {
            self.change(
                context,
                app,
                Change::VolumeStep(ticks as f32 * step / 100.0),
            );
        }
    }

    /// Runs in its own task so a slow audio service does not block the main loop.
    fn change(&self, context: &str, app: String, change: Change) {
        let (host, context) = (self.host.clone(), context.to_owned());
        tokio::spawn(async move {
            if let Err(err) = platform::change(app, change).await {
                host.alert(&context);
                host.log(format!("Lautstärke konnte nicht geändert werden: {err:#}"));
            }
        });
    }

    fn on_request(&self, req: Value) {
        let host = self.host.clone();
        tokio::spawn(async move {
            let options = match req["request"].as_str() {
                Some("apps") => platform::snapshot()
                    .await
                    .context("Audiositzungen nicht abrufbar")
                    .map(|s| app_options(&s.sessions)),
                other => Err(anyhow::anyhow!("unbekannte Anfrage {other:?}")),
            };
            host.reply_options(&req, options);
        });
    }

    fn on_tick(&mut self) {
        if self.busy || !(self.contexts.any("app") || self.contexts.any("focused")) {
            return;
        }
        self.busy = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tx.send(Msg::Snapshot(platform::snapshot().await)).ok();
        });
    }

    fn on_snapshot(&mut self, result: anyhow::Result<Snapshot>) {
        self.busy = false;
        self.snapshot = match result {
            Ok(snapshot) => {
                self.failing = false;
                snapshot
            }
            Err(err) => {
                // Log once per failure, not on every second.
                if !self.failing {
                    self.host
                        .log(format!("Audiositzungen nicht abrufbar: {err:#}"));
                }
                self.failing = true;
                Snapshot::default()
            }
        };

        let instances: Vec<(String, Instance)> = self
            .contexts
            .iter()
            .map(|(context, inst)| (context.clone(), inst.clone()))
            .collect();
        for (context, inst) in instances {
            if !matches!(inst.kind(), "app" | "focused") {
                continue;
            }
            let session = self.target(&inst).and_then(|app| {
                self.snapshot
                    .sessions
                    .iter()
                    .find(|s| same_app(&s.app, &app))
            });
            let (state, title) = match session {
                Some(session) => (u16::from(session.muted), session_title(session)),
                None => (0, ERROR_TITLE.to_owned()),
            };
            self.contexts.show(&self.host, &context, state, &title);
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Snapshot(result) => self.on_snapshot(result),
        }
    }
}

/// Process names compare without case and without `.exe`.
pub fn same_app(a: &str, b: &str) -> bool {
    app_key(a) == app_key(b)
}

fn app_key(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// Dropdown entries: each running app once, sorted by name.
fn app_options(sessions: &[Session]) -> Vec<(String, String)> {
    let mut apps: Vec<String> = sessions.iter().map(|s| s.app.clone()).collect();
    apps.sort_by_key(|app| app_key(app));
    apps.dedup_by(|a, b| same_app(a, b));
    apps.into_iter().map(|app| (app.clone(), app)).collect()
}

fn session_title(session: &Session) -> String {
    if session.muted {
        MUTED_TITLE.to_owned()
    } else {
        format!("{:.0} %", session.volume * 100.0)
    }
}

/// Keeps a volume in 0.0 to 1.0 after a dial step.
pub fn step_volume(volume: f32, delta: f32) -> f32 {
    (volume + delta).clamp(0.0, 1.0)
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
            _ = tick.tick() => plugin.on_tick(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(app: &str, volume: f32, muted: bool) -> Session {
        Session {
            app: app.to_owned(),
            volume,
            muted,
        }
    }

    #[test]
    fn app_names_ignore_case_and_exe() {
        assert!(same_app("Discord.exe", "discord"));
        assert!(same_app("spotify", "SPOTIFY.EXE"));
        assert!(!same_app("Discord.exe", "Discord Helper.exe"));
    }

    #[test]
    fn options_list_each_app_once() {
        let sessions = [
            session("obs64.exe", 1.0, false),
            session("Discord.exe", 0.5, false),
            session("discord", 0.5, false),
        ];
        assert_eq!(
            app_options(&sessions),
            vec![
                ("Discord.exe".to_owned(), "Discord.exe".to_owned()),
                ("obs64.exe".to_owned(), "obs64.exe".to_owned()),
            ]
        );
    }

    #[test]
    fn volume_stays_between_zero_and_full() {
        assert_eq!(step_volume(0.98, 0.04), 1.0);
        assert_eq!(step_volume(0.02, -0.05), 0.0);
        assert!((step_volume(0.5, 0.02) - 0.52).abs() < 1e-6);
    }

    #[test]
    fn title_shows_volume_or_mute() {
        assert_eq!(session_title(&session("a", 0.45, false)), "45 %");
        assert_eq!(session_title(&session("a", 0.45, true)), MUTED_TITLE);
    }
}

#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 media plugin: shows title and cover of the running playback and
//! controls it (Windows: system media transport controls, Linux: MPRIS; see
//! `docs/PLUGIN_MEDIA.md`).

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use crate::{Command, Snapshot};

    pub async fn snapshot() -> anyhow::Result<Option<Snapshot>> {
        anyhow::bail!("nicht unterstützt")
    }

    pub async fn command(_: Command) -> anyhow::Result<()> {
        anyhow::bail!("nicht unterstützt")
    }
}

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    io::Cursor,
    time::Duration,
};

use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use n3_plugin_sdk::{Contexts, Host, Instance};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_secs(1);
const COVER_SIZE: u32 = 144;
const TITLE_CHARS: usize = 14;
const SEEK_SECS: f64 = 5.0;
const ERROR_TITLE: &str = "–";

#[derive(Clone, Copy)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
    Seek(f64),
}

pub struct Snapshot {
    pub title: String,
    pub playing: bool,
    pub cover: Option<Vec<u8>>,
}

enum Msg {
    Host(Value),
    Snapshot(anyhow::Result<Option<Snapshot>>),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    busy: bool,
    failing: bool,
    cover_hash: Option<u64>,
    cover_url: Option<String>,
    /// Cover hash last sent per context; `None` means no cover (default image).
    sent: HashMap<String, Option<u64>>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            busy: false,
            failing: false,
            cover_hash: None,
            cover_url: None,
            sent: HashMap::new(),
        }
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.sent.remove(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => self.rotate(&context, msg["payload"]["ticks"].as_i64().unwrap_or(0)),
            _ => {}
        }
    }

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        match inst.kind() {
            "playpause" | "nowplaying" => self.command(context, Command::PlayPause),
            "next" => self.command(context, Command::Next),
            "previous" => self.command(context, Command::Previous),
            _ => {}
        }
    }

    fn rotate(&self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        if inst.kind() != "nowplaying" || ticks == 0 {
            return;
        }
        if inst.text_or("dial", "seek") == "track" {
            let cmd = if ticks > 0 {
                Command::Next
            } else {
                Command::Previous
            };
            for _ in 0..ticks.unsigned_abs() {
                self.command(context, cmd);
            }
        } else {
            self.command(context, Command::Seek(ticks as f64 * SEEK_SECS));
        }
    }

    /// Runs in its own task so a slow player does not block the main loop.
    fn command(&self, context: &str, cmd: Command) {
        let (host, context) = (self.host.clone(), context.to_owned());
        tokio::spawn(async move {
            if let Err(err) = platform::command(cmd).await {
                host.alert(&context);
                host.log(format!("Medienbefehl fehlgeschlagen: {err:#}"));
            }
        });
    }

    fn on_tick(&mut self) {
        if self.busy || self.contexts.iter().next().is_none() {
            return;
        }
        self.busy = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tx.send(Msg::Snapshot(platform::snapshot().await)).ok();
        });
    }

    fn on_snapshot(&mut self, result: anyhow::Result<Option<Snapshot>>) {
        self.busy = false;
        let snapshot = match result {
            Ok(snapshot) => {
                self.failing = false;
                snapshot
            }
            Err(err) => {
                // Log once per failure, not on every second.
                if !self.failing {
                    self.host.log(format!("Medien nicht abrufbar: {err:#}"));
                }
                self.failing = true;
                None
            }
        };
        let (playing, title, cover) = match snapshot {
            Some(s) => (s.playing, shorten(&s.title), s.cover),
            None => (false, ERROR_TITLE.to_owned(), None),
        };

        let hash = cover.as_deref().map(hash_bytes);
        if hash != self.cover_hash {
            self.cover_url = cover
                .as_deref()
                .and_then(|bytes| match cover_data_url(bytes) {
                    Ok(url) => Some(url),
                    Err(err) => {
                        self.host.log(format!("Cover nicht anzeigbar: {err:#}"));
                        None
                    }
                });
            self.cover_hash = hash;
        }

        self.sent
            .retain(|context, _| self.contexts.get(context).is_some());
        let instances: Vec<(String, Instance)> = self
            .contexts
            .iter()
            .map(|(context, inst)| (context.clone(), inst.clone()))
            .collect();
        for (context, inst) in instances {
            let (state, title, wants_cover) = match inst.kind() {
                "playpause" => (u16::from(playing), "", inst.flag("showCover", false)),
                "nowplaying" => (0, title.as_str(), true),
                _ => continue,
            };
            self.contexts.show(&self.host, &context, state, title);
            let want = if wants_cover { self.cover_hash } else { None };
            if self.sent.get(&context) != Some(&want) {
                let image = if want.is_some() {
                    self.cover_url.clone()
                } else {
                    None
                };
                self.host.send(json!({
                    "event": "setImage",
                    "context": context,
                    "payload": { "image": image },
                }));
                self.sent.insert(context, want);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Snapshot(result) => self.on_snapshot(result),
        }
    }
}

/// Cuts a title to the key width; counts characters, not bytes.
fn shorten(title: &str) -> String {
    let title = title.trim();
    if title.chars().count() <= TITLE_CHARS {
        return title.to_owned();
    }
    let mut short: String = title.chars().take(TITLE_CHARS - 1).collect();
    short.push('…');
    short
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Square 144×144 PNG as data URL, the format `setImage` takes.
fn cover_data_url(bytes: &[u8]) -> anyhow::Result<String> {
    let image = image::load_from_memory(bytes).context("Bild nicht lesbar")?;
    let square = image.resize_to_fill(
        COVER_SIZE,
        COVER_SIZE,
        image::imageops::FilterType::Triangle,
    );
    let mut png = Vec::new();
    square.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
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

    #[test]
    fn title_fits_key() {
        assert_eq!(shorten("  Kurz  "), "Kurz");
        assert_eq!(shorten("12345678901234"), "12345678901234");
        let short = shorten("ÄÖÜäöüßÄÖÜäöüßÄÖÜ");
        assert_eq!(short.chars().count(), TITLE_CHARS);
        assert!(short.ends_with('…'));
    }

    #[test]
    fn same_cover_same_hash() {
        assert_eq!(hash_bytes(b"cover"), hash_bytes(b"cover"));
        assert_ne!(hash_bytes(b"cover"), hash_bytes(b"other"));
    }

    #[test]
    fn cover_becomes_square_png() {
        let source = image::RgbImage::from_pixel(300, 200, image::Rgb([200, 40, 40]));
        let mut bytes = Vec::new();
        source
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let url = cover_data_url(&bytes).unwrap();
        let png = STANDARD
            .decode(url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (COVER_SIZE, COVER_SIZE)
        );
        assert!(cover_data_url(b"kein Bild").is_err());
    }
}

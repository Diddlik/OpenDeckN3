//! Actions implemented directly in the service (no plugin process needed).
//!
//! The set follows OpenDeck's starter pack (run command, open URL, simulate
//! input, switch profile, brightness) plus volume, media keys, launching
//! programs and typing text. Each action describes its settings with a small
//! schema (`settingsSchema` in the catalog) that the UI renders as a form.

use std::sync::LazyLock;

use enigo::Key;
use image::DynamicImage;
use n3_core::{ActionInstance, Controller, InputEvent};
use n3_plugin::BUILTIN_PLUGIN;
use serde_json::{Map, Value, json};

use crate::{
    app::App,
    system::{
        input::InputJob,
        launch::{launch, run_command},
        shortcut,
    },
};

const PREFIX: &str = "opendeckn3.builtin.";
pub const BRIGHTNESS: &str = "opendeckn3.builtin.brightness";
pub const SWITCH_PROFILE: &str = "opendeckn3.builtin.profile";
pub const HOTKEY: &str = "opendeckn3.builtin.hotkey";
pub const TEXT: &str = "opendeckn3.builtin.text";
pub const VOLUME: &str = "opendeckn3.builtin.volume";
pub const MEDIA: &str = "opendeckn3.builtin.media";
pub const LAUNCH: &str = "opendeckn3.builtin.launch";
pub const URL: &str = "opendeckn3.builtin.url";
pub const COMMAND: &str = "opendeckn3.builtin.command";

const BRIGHTNESS_PRESETS: [u8; 5] = [10, 25, 50, 75, 100];
const BRIGHTNESS_STEP: i16 = 5;

const KEYPAD: &[&str] = &["Keypad"];
const ENCODER: &[&str] = &["Encoder"];
const BOTH: &[&str] = &["Keypad", "Encoder"];

/// Catalog of all built-in actions (UUID, name, tooltip, controllers, settings schema).
static ACTIONS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let field = |key: &str, label: &str, kind: &str, controllers: &[&str]| json!({ "key": key, "label": label, "type": kind, "controllers": controllers });
    let with = |mut f: Value, extra: Value| {
        if let (Some(f), Some(extra)) = (f.as_object_mut(), extra.as_object()) {
            f.extend(extra.clone());
        }
        f
    };
    let action =
        |uuid: &str, name: &str, tooltip: &str, controllers: &[&str], schema: Vec<Value>| {
            json!({
                "uuid": uuid, "name": name, "tooltip": tooltip,
                "controllers": controllers, "icon": null, "settingsSchema": schema,
            })
        };
    let shortcut_help = "Ins Feld klicken und die Tasten drücken, z. B. Strg+Umschalt+M – wird sofort übernommen. Esc bricht ab.";
    vec![
        action(
            HOTKEY,
            "Tastenkürzel",
            "Drückt eine Tastenkombination, z. B. Ctrl+Shift+M",
            BOTH,
            vec![
                with(
                    field("shortcut", "Tastenkürzel", "shortcut", KEYPAD),
                    json!({ "help": shortcut_help }),
                ),
                with(
                    field("clockwise", "Drehen rechts", "shortcut", ENCODER),
                    json!({ "help": "Wird je Raste beim Rechtsdrehen gedrückt" }),
                ),
                with(
                    field("anticlockwise", "Drehen links", "shortcut", ENCODER),
                    json!({ "help": "Wird je Raste beim Linksdrehen gedrückt" }),
                ),
            ],
        ),
        action(
            VOLUME,
            "Lautstärke",
            "Taste: lauter, leiser oder stumm · Drehregler: drehen = Lautstärke",
            BOTH,
            vec![
                with(
                    field("mode", "Funktion", "select", KEYPAD),
                    json!({
                        "default": "mute",
                        "options": [["mute", "Stumm an/aus"], ["up", "Lauter"], ["down", "Leiser"]],
                    }),
                ),
                with(
                    field("step", "Schritte", "number", BOTH),
                    json!({
                        "default": 1, "min": 1, "max": 10,
                        "help": "Lautstärkestufen pro Druck bzw. Raste (unter Windows je ca. 2 %)",
                    }),
                ),
            ],
        ),
        action(
            MEDIA,
            "Medien",
            "Taste: Play/Pause, vor, zurück, Stopp · Drehregler: drehen = Titel vor/zurück",
            BOTH,
            vec![with(
                field("mode", "Funktion", "select", KEYPAD),
                json!({
                    "default": "playpause",
                    "options": [["playpause", "Play/Pause"], ["next", "Nächster Titel"], ["previous", "Vorheriger Titel"], ["stop", "Stopp"]],
                }),
            )],
        ),
        action(
            LAUNCH,
            "Programm öffnen",
            "Startet ein Programm oder öffnet eine Datei bzw. einen Ordner",
            KEYPAD,
            vec![
                with(
                    field("path", "Programm / Datei", "file", KEYPAD),
                    json!({
                        "placeholder": "z. B. C:\\Windows\\notepad.exe oder spotify",
                        "source": "apps",
                        "help": "Installiertes Programm aus der Liste wählen – oder Pfad eintragen bzw. „Durchsuchen …“ (Programm, Datei oder Ordner).",
                    }),
                ),
                with(
                    field("args", "Argumente", "text", KEYPAD),
                    json!({ "placeholder": "optional, nur bei Programmen" }),
                ),
            ],
        ),
        action(
            URL,
            "Website öffnen",
            "Öffnet eine Adresse im Standardbrowser",
            KEYPAD,
            vec![with(
                field("url", "Adresse", "text", KEYPAD),
                json!({ "placeholder": "https://…" }),
            )],
        ),
        action(
            COMMAND,
            "Befehl ausführen",
            "Führt einen Befehl in der Kommandozeile aus",
            BOTH,
            vec![
                with(
                    field("command", "Befehl", "text", KEYPAD),
                    json!({ "placeholder": "z. B. shutdown /h", "help": "Windows: cmd /C … · Linux: sh -c …" }),
                ),
                with(
                    field("clockwise", "Drehen rechts", "text", ENCODER),
                    json!({ "placeholder": "%d = Anzahl Rasten" }),
                ),
                with(
                    field("anticlockwise", "Drehen links", "text", ENCODER),
                    json!({ "placeholder": "%d = Anzahl Rasten" }),
                ),
            ],
        ),
        action(
            TEXT,
            "Text eingeben",
            "Tippt einen Text, z. B. eine Signatur oder E-Mail-Adresse",
            KEYPAD,
            vec![field("text", "Text", "textarea", KEYPAD)],
        ),
        action(
            SWITCH_PROFILE,
            "Profil wechseln",
            "Wechselt zu einem anderen Profil",
            KEYPAD,
            vec![field("profile", "Profil", "profile", KEYPAD)],
        ),
        action(
            BRIGHTNESS,
            "Helligkeit",
            "Taste: Helligkeitsstufen durchschalten, Drehregler: stufenlos",
            BOTH,
            vec![],
        ),
    ]
});

pub fn catalog_entry() -> Value {
    json!({
        "uuid": BUILTIN_PLUGIN,
        "name": "OpenDeckN3",
        "author": "OpenDeckN3",
        "version": env!("CARGO_PKG_VERSION"),
        "category": "System",
        "connected": true,
        "actions": *ACTIONS,
    })
}

pub fn exists(action: &str) -> bool {
    ACTIONS.iter().any(|a| a["uuid"] == action)
}

/// Initial settings from the schema defaults.
pub fn default_settings(action: &str) -> Value {
    match ACTIONS.iter().find(|a| a["uuid"] == action) {
        Some(spec) => schema_defaults(&spec["settingsSchema"]),
        None => Value::Object(Map::new()),
    }
}

/// `{key: default}` for all fields of a settings schema that have a default.
pub fn schema_defaults(schema: &Value) -> Value {
    let mut settings = Map::new();
    for field in schema.as_array().into_iter().flatten() {
        if let (Some(key), Some(default)) = (field["key"].as_str(), field.get("default")) {
            settings.insert(key.to_owned(), default.clone());
        }
    }
    Value::Object(settings)
}

fn setting<'a>(instance: &'a ActionInstance, key: &str) -> &'a str {
    instance
        .settings
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
}

fn step(instance: &ActionInstance) -> u32 {
    instance
        .settings
        .get("step")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(1)
        .clamp(1, 10) as u32
}

/// Short label drawn on the key when the user set no title.
pub fn default_label(instance: &ActionInstance) -> Option<String> {
    let label = match instance.action.as_str() {
        HOTKEY => setting(instance, "shortcut").to_owned(),
        SWITCH_PROFILE => format!("→ {}", setting(instance, "profile")),
        LAUNCH => {
            let path = setting(instance, "path").trim_matches('"');
            // Application-menu entries carry their display name.
            if path.ends_with(".desktop")
                && let Some(entry) =
                    crate::system::apps::DesktopEntry::read(std::path::Path::new(path))
                && !entry.name.is_empty()
            {
                return Some(entry.name);
            }
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            name.rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(name)
                .to_owned()
        }
        URL => {
            let url = setting(instance, "url");
            let host = url.split("://").nth(1).unwrap_or(url);
            host.split(['/', '?', '#'])
                .next()
                .unwrap_or(host)
                .trim_start_matches("www.")
                .to_owned()
        }
        _ => return None,
    };
    (!label.is_empty() && label != "→ ").then_some(label)
}

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        fn icon_bytes(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../assets/builtin/", $name, ".png"))),)*
                _ => None,
            }
        }
    };
}
icons!(
    "brightness",
    "profile",
    "hotkey",
    "text",
    "volume-up",
    "volume-down",
    "volume-mute",
    "media-playpause",
    "media-next",
    "media-previous",
    "media-stop",
    "launch",
    "url",
    "command",
);

/// Key image for a built-in action (generated by `tools/make-builtin-icons.cjs`).
pub fn icon(instance: &ActionInstance) -> Option<DynamicImage> {
    let name = match instance.action.as_str() {
        VOLUME => match setting(instance, "mode") {
            "up" => "volume-up",
            "down" => "volume-down",
            _ => "volume-mute",
        },
        MEDIA => match setting(instance, "mode") {
            "next" => "media-next",
            "previous" => "media-previous",
            "stop" => "media-stop",
            _ => "media-playpause",
        },
        other => other.strip_prefix(PREFIX)?,
    };
    image::load_from_memory(icon_bytes(name)?).ok()
}

fn next_preset(current: u8) -> u8 {
    BRIGHTNESS_PRESETS
        .iter()
        .copied()
        .find(|p| *p > current)
        .unwrap_or(BRIGHTNESS_PRESETS[0])
}

/// What a built-in action does in response to one input.
enum Effect {
    None,
    Input(InputJob),
    Launch { path: String, args: String },
    Url(String),
    Command(String),
    Brightness(u8),
    Profile(String),
}

fn effect(instance: &ActionInstance, input: &InputEvent, brightness: u8) -> anyhow::Result<Effect> {
    use InputEvent::*;
    // Knob presses reach built-ins only as KeyDown of their button assignment;
    // rotation assignments react to turning only.
    let pressed = matches!(input, KeyDown { .. });
    let twist = match input {
        EncoderTwist { ticks, .. } => *ticks,
        _ => 0,
    };
    let shortcut = |s: &str| -> anyhow::Result<Effect> {
        let chords = shortcut::parse(s).map_err(anyhow::Error::msg)?;
        Ok(if chords.is_empty() {
            Effect::None
        } else {
            Effect::Input(InputJob::Chords(chords))
        })
    };
    let turn = if twist > 0 {
        "clockwise"
    } else {
        "anticlockwise"
    };

    Ok(match instance.action.as_str() {
        HOTKEY if pressed => shortcut(setting(instance, "shortcut"))?,
        HOTKEY if twist != 0 => {
            let s = setting(instance, turn);
            match shortcut(s)? {
                Effect::Input(InputJob::Chords(chords)) => {
                    let repeated = (0..twist.unsigned_abs())
                        .flat_map(|_| chords.clone())
                        .collect();
                    Effect::Input(InputJob::Chords(repeated))
                }
                other => other,
            }
        }
        VOLUME => {
            let n = step(instance);
            match input {
                EncoderTwist { ticks, .. } => {
                    let key = if *ticks > 0 {
                        Key::VolumeUp
                    } else {
                        Key::VolumeDown
                    };
                    Effect::Input(InputJob::Tap(key, n * u32::from(ticks.unsigned_abs())))
                }
                KeyDown { .. } => match setting(instance, "mode") {
                    "up" => Effect::Input(InputJob::Tap(Key::VolumeUp, n)),
                    "down" => Effect::Input(InputJob::Tap(Key::VolumeDown, n)),
                    _ => Effect::Input(InputJob::Tap(Key::VolumeMute, 1)),
                },
                _ => Effect::None,
            }
        }
        MEDIA => {
            let key = match input {
                EncoderTwist { ticks, .. } if *ticks > 0 => Key::MediaNextTrack,
                EncoderTwist { .. } => Key::MediaPrevTrack,
                KeyDown { .. } => match setting(instance, "mode") {
                    "next" => Key::MediaNextTrack,
                    "previous" => Key::MediaPrevTrack,
                    #[cfg(not(target_os = "macos"))]
                    "stop" => Key::MediaStop,
                    _ => Key::MediaPlayPause,
                },
                _ => return Ok(Effect::None),
            };
            Effect::Input(InputJob::Tap(key, 1))
        }
        TEXT if pressed => match setting(instance, "text") {
            "" => Effect::None,
            text => Effect::Input(InputJob::Text(text.to_owned())),
        },
        LAUNCH if pressed => Effect::Launch {
            path: setting(instance, "path").to_owned(),
            args: setting(instance, "args").to_owned(),
        },
        URL if pressed => Effect::Url(setting(instance, "url").to_owned()),
        COMMAND if pressed => Effect::Command(setting(instance, "command").to_owned()),
        COMMAND if twist != 0 => {
            let cmd = setting(instance, turn);
            Effect::Command(cmd.replace("%d", &twist.unsigned_abs().to_string()))
        }
        BRIGHTNESS => match input {
            KeyDown { .. } => Effect::Brightness(next_preset(brightness)),
            EncoderTwist { ticks, .. } => {
                Effect::Brightness((brightness as i16 + ticks * BRIGHTNESS_STEP).clamp(0, 100) as u8)
            }
            _ => Effect::None,
        },
        SWITCH_PROFILE if pressed => match setting(instance, "profile") {
            "" => anyhow::bail!("kein Profil gewählt"),
            profile => Effect::Profile(profile.to_owned()),
        },
        _ => Effect::None,
    })
}

/// Handles an input on a slot bound to a built-in action. Failures are
/// reported to the UI (`actionError`) instead of being returned.
pub async fn on_input(app: &mut App, device: &str, input: InputEvent) {
    let (controller, position) = match input {
        InputEvent::KeyDown { key } | InputEvent::KeyUp { key } => (Controller::Keypad, key),
        InputEvent::EncoderDown { encoder }
        | InputEvent::EncoderUp { encoder }
        | InputEvent::EncoderTwist { encoder, .. } => (Controller::Encoder, encoder),
    };
    let report = {
        let ui = app.ui_sender();
        let device = device.to_owned();
        move |message: String| {
            tracing::warn!(%device, ?controller, position, "{message}");
            ui.send(json!({
                "event": "actionError", "device": device,
                "controller": controller, "position": position, "message": message,
            }))
            .ok();
        }
    };

    let Some(state) = app.devices.get(device) else {
        return;
    };
    let Some(instance) = state.profile.slots(controller).get(&position) else {
        return;
    };
    let effect = match effect(instance, &input, state.config.brightness) {
        Ok(effect) => effect,
        Err(err) => return report(format!("{err:#}")),
    };

    let result = match effect {
        Effect::None => Ok(()),
        Effect::Input(job) => {
            let input = app.input();
            tokio::spawn(async move {
                if let Err(err) = input.run(job).await {
                    report(err);
                }
            });
            return;
        }
        Effect::Launch { path, args } => launch(&path, &args),
        Effect::Url(url) => crate::app::open_url(&url),
        Effect::Command(cmd) if cmd.is_empty() => Ok(()),
        Effect::Command(cmd) => run_command(&cmd),
        Effect::Brightness(value) => app.set_brightness(device, value).await,
        Effect::Profile(profile) => app.switch_profile(device, &profile).await,
    };
    if let Err(err) = result {
        report(format!("{err:#}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(action: &str, settings: Value) -> ActionInstance {
        let mut i = ActionInstance::new(BUILTIN_PLUGIN, action);
        i.settings = settings;
        i
    }

    #[test]
    fn presets_wrap_around() {
        assert_eq!(next_preset(0), 10);
        assert_eq!(next_preset(50), 75);
        assert_eq!(next_preset(60), 75);
        assert_eq!(next_preset(100), 10);
    }

    #[test]
    fn defaults_come_from_schema() {
        assert_eq!(
            default_settings(VOLUME),
            json!({ "mode": "mute", "step": 1 })
        );
        assert_eq!(default_settings(HOTKEY), json!({}));
        assert!(exists(MEDIA) && !exists("opendeckn3.builtin.nope"));
    }

    #[test]
    fn every_action_has_an_icon() {
        for action in ACTIONS.iter() {
            let i = instance(
                action["uuid"].as_str().unwrap(),
                default_settings(action["uuid"].as_str().unwrap()),
            );
            assert!(icon(&i).is_some(), "missing icon for {}", i.action);
        }
    }

    #[test]
    fn labels() {
        let l = |a, s| default_label(&instance(a, s));
        assert_eq!(
            l(HOTKEY, json!({ "shortcut": "Ctrl+C" })).as_deref(),
            Some("Ctrl+C")
        );
        assert_eq!(
            l(LAUNCH, json!({ "path": "C:\\Programs\\Spotify.exe" })).as_deref(),
            Some("Spotify")
        );
        assert_eq!(
            l(URL, json!({ "url": "https://www.github.com/x" })).as_deref(),
            Some("github.com")
        );
        assert_eq!(l(SWITCH_PROFILE, json!({})), None);
        assert_eq!(l(VOLUME, json!({ "mode": "up" })), None);
    }

    #[test]
    fn effects() {
        let down = InputEvent::KeyDown { key: 0 };
        let twist = |ticks| InputEvent::EncoderTwist { encoder: 1, ticks };
        let hotkey = instance(
            HOTKEY,
            json!({ "shortcut": "Ctrl+C", "clockwise": "Ctrl+Plus" }),
        );
        assert!(
            matches!(effect(&hotkey, &down, 50).unwrap(), Effect::Input(InputJob::Chords(c)) if c.len() == 1)
        );
        assert!(
            matches!(effect(&hotkey, &twist(3), 50).unwrap(), Effect::Input(InputJob::Chords(c)) if c.len() == 3)
        );
        assert!(matches!(
            effect(&hotkey, &twist(-1), 50).unwrap(),
            Effect::None
        ));
        assert!(
            effect(
                &instance(HOTKEY, json!({ "shortcut": "Ctrl+Nope" })),
                &down,
                50
            )
            .is_err()
        );

        let volume = instance(VOLUME, json!({ "mode": "down", "step": 2 }));
        assert!(matches!(
            effect(&volume, &down, 50).unwrap(),
            Effect::Input(InputJob::Tap(Key::VolumeDown, 2))
        ));
        assert!(matches!(
            effect(&volume, &twist(3), 50).unwrap(),
            Effect::Input(InputJob::Tap(Key::VolumeUp, 6))
        ));

        // Rotation assignments ignore knob presses (those belong to the button assignment).
        let press = InputEvent::EncoderDown { encoder: 1 };
        assert!(matches!(effect(&volume, &press, 50).unwrap(), Effect::None));
        assert!(matches!(effect(&hotkey, &press, 50).unwrap(), Effect::None));
        for action in [LAUNCH, URL, TEXT] {
            let spec = ACTIONS.iter().find(|a| a["uuid"] == action).unwrap();
            assert_eq!(
                spec["controllers"],
                json!(["Keypad"]),
                "{action} has no rotation function"
            );
        }

        let cmd = instance(COMMAND, json!({ "clockwise": "vol +%d" }));
        assert!(
            matches!(effect(&cmd, &twist(4), 50).unwrap(), Effect::Command(c) if c == "vol +4")
        );
        assert!(matches!(
            effect(&instance(BRIGHTNESS, json!({})), &twist(-2), 50).unwrap(),
            Effect::Brightness(40)
        ));
    }
}

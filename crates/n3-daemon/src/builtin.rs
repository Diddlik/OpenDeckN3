//! Actions implemented directly in the daemon (no plugin process needed).
//!
//! - `opendeckn3.builtin.brightness`: key press cycles brightness presets,
//!   encoder twist changes it in 5 % steps.
//! - `opendeckn3.builtin.profile`: key press switches to the profile named in
//!   `settings.profile`.

use n3_core::{Controller, InputEvent};
use n3_plugin::BUILTIN_PLUGIN;
use serde_json::{Value, json};

use crate::app::App;

pub const BRIGHTNESS: &str = "opendeckn3.builtin.brightness";
pub const SWITCH_PROFILE: &str = "opendeckn3.builtin.profile";

const BRIGHTNESS_PRESETS: [u8; 5] = [10, 25, 50, 75, 100];
const BRIGHTNESS_STEP: i16 = 5;

pub fn catalog_entry() -> Value {
    json!({
        "uuid": BUILTIN_PLUGIN,
        "name": "OpenDeckN3",
        "author": "OpenDeckN3",
        "version": env!("CARGO_PKG_VERSION"),
        "category": "System",
        "connected": true,
        "actions": [
            {
                "uuid": BRIGHTNESS,
                "name": "Helligkeit",
                "tooltip": "Taste: Helligkeitsstufen durchschalten, Drehregler: stufenlos",
                "controllers": ["Keypad", "Encoder"],
                "icon": null,
            },
            {
                "uuid": SWITCH_PROFILE,
                "name": "Profil wechseln",
                "tooltip": "Wechselt zum Profil aus den Einstellungen (settings.profile)",
                "controllers": ["Keypad"],
                "icon": null,
            },
        ],
    })
}

fn next_preset(current: u8) -> u8 {
    BRIGHTNESS_PRESETS
        .iter()
        .copied()
        .find(|p| *p > current)
        .unwrap_or(BRIGHTNESS_PRESETS[0])
}

pub async fn on_input(app: &mut App, device: &str, input: InputEvent) -> anyhow::Result<()> {
    let (controller, position) = match input {
        InputEvent::KeyDown { key } => (Controller::Keypad, key),
        InputEvent::EncoderTwist { encoder, .. } => (Controller::Encoder, encoder),
        // Releases and encoder presses are not used by built-ins yet.
        _ => return Ok(()),
    };
    let Some(state) = app.devices.get(device) else {
        return Ok(());
    };
    let Some(instance) = state.profile.slots(controller).get(&position) else {
        return Ok(());
    };
    let brightness = state.config.brightness;

    match (instance.action.as_str(), input) {
        (BRIGHTNESS, InputEvent::KeyDown { .. }) => {
            app.set_brightness(device, next_preset(brightness)).await
        }
        (BRIGHTNESS, InputEvent::EncoderTwist { ticks, .. }) => {
            let value = (brightness as i16 + ticks * BRIGHTNESS_STEP).clamp(0, 100);
            app.set_brightness(device, value as u8).await
        }
        (SWITCH_PROFILE, InputEvent::KeyDown { .. }) => {
            let target = instance
                .settings
                .get("profile")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match target {
                Some(profile) => app.switch_profile(device, &profile).await,
                None => anyhow::bail!("profile action without settings.profile"),
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_wrap_around() {
        assert_eq!(next_preset(0), 10);
        assert_eq!(next_preset(50), 75);
        assert_eq!(next_preset(60), 75);
        assert_eq!(next_preset(100), 10);
    }
}

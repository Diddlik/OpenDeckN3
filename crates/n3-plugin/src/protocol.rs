//! Wire format between host and plugins (Stream Deck SDK subset).
//!
//! See `docs/PLUGIN_API.md` for the full description.

use n3_core::{Controller, Coordinates, DeviceInfo};
use serde::Deserialize;
use serde_json::{Value, json};

/// First message every plugin must send after connecting.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum RegisterEvent {
    RegisterPlugin { uuid: String },
}

/// Messages a plugin may send to the host.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum InboundEvent {
    SetSettings {
        context: String,
        payload: Value,
    },
    GetSettings {
        context: String,
    },
    SetGlobalSettings {
        context: String,
        payload: Value,
    },
    GetGlobalSettings {
        context: String,
    },
    SetTitle {
        context: String,
        payload: SetTitlePayload,
    },
    SetImage {
        context: String,
        payload: SetImagePayload,
    },
    SetState {
        context: String,
        payload: SetStatePayload,
    },
    ShowOk {
        context: String,
    },
    ShowAlert {
        context: String,
    },
    LogMessage {
        payload: LogMessagePayload,
    },
    OpenUrl {
        payload: OpenUrlPayload,
    },
    /// Any event this host does not implement yet.
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Deserialize)]
pub struct SetTitlePayload {
    pub title: Option<String>,
    pub state: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub struct SetImagePayload {
    /// Data URL (`data:image/png;base64,...`) or `None` to reset.
    pub image: Option<String>,
    pub state: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub struct SetStatePayload {
    pub state: u16,
}

#[derive(Debug, Deserialize)]
pub struct LogMessagePayload {
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenUrlPayload {
    pub url: String,
}

/// Everything the host knows about a slot when talking to a plugin.
pub struct SlotRef<'a> {
    pub action: &'a str,
    pub context: &'a str,
    pub device: &'a str,
    pub controller: Controller,
    pub coordinates: Coordinates,
    pub settings: &'a Value,
    pub state: u16,
}

fn controller_name(controller: Controller) -> &'static str {
    match controller {
        Controller::Keypad => "Keypad",
        Controller::Encoder => "Encoder",
    }
}

impl SlotRef<'_> {
    fn base_payload(&self) -> serde_json::Map<String, Value> {
        let mut payload = serde_json::Map::new();
        payload.insert("settings".into(), self.settings.clone());
        payload.insert("coordinates".into(), json!(self.coordinates));
        payload.insert("controller".into(), json!(controller_name(self.controller)));
        payload.insert("state".into(), json!(self.state));
        payload.insert("isInMultiAction".into(), json!(false));
        payload
    }

    fn event(&self, event: &str, payload: serde_json::Map<String, Value>) -> Value {
        json!({
            "event": event,
            "action": self.action,
            "context": self.context,
            "device": self.device,
            "payload": payload,
        })
    }

    /// `keyDown`, `keyUp`, `dialDown`, `dialUp`, `willAppear`,
    /// `willDisappear`, `didReceiveSettings`.
    pub fn simple(&self, event: &str) -> Value {
        self.event(event, self.base_payload())
    }

    pub fn dial_rotate(&self, ticks: i16, pressed: bool) -> Value {
        let mut payload = self.base_payload();
        payload.insert("ticks".into(), json!(ticks));
        payload.insert("pressed".into(), json!(pressed));
        self.event("dialRotate", payload)
    }
}

pub fn device_did_connect(device: &DeviceInfo) -> Value {
    json!({
        "event": "deviceDidConnect",
        "device": device.id,
        "deviceInfo": {
            "name": device.name,
            // 7 = "StreamDeckPlus"-like (keys + dials) in the Elgato enum.
            "type": 7,
            "size": { "columns": device.layout.columns, "rows": device.layout.rows },
        },
    })
}

pub fn device_did_disconnect(device: &str) -> Value {
    json!({ "event": "deviceDidDisconnect", "device": device })
}

pub fn did_receive_global_settings(settings: &Value) -> Value {
    json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": settings } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_set_image_and_unknown_events() {
        let ev: InboundEvent = serde_json::from_str(
            r#"{"event":"setImage","context":"c","payload":{"image":"data:image/png;base64,AA=="}}"#,
        )
        .unwrap();
        assert!(matches!(ev, InboundEvent::SetImage { .. }));

        let ev: InboundEvent =
            serde_json::from_str(r#"{"event":"switchToProfile","context":"x"}"#).unwrap();
        assert!(matches!(ev, InboundEvent::Unsupported));
    }

    #[test]
    fn builds_dial_rotate() {
        let settings = json!({"a": 1});
        let slot = SlotRef {
            action: "com.example.a",
            context: "ctx",
            device: "n3-1",
            controller: Controller::Encoder,
            coordinates: Coordinates { column: 1, row: 0 },
            settings: &settings,
            state: 0,
        };
        let v = slot.dial_rotate(-2, false);
        assert_eq!(v["event"], "dialRotate");
        assert_eq!(v["payload"]["ticks"], -2);
        assert_eq!(v["payload"]["controller"], "Encoder");
    }
}

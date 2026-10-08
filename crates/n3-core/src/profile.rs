use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Controller;

/// An action placed on a key or encoder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionInstance {
    /// UUID of the plugin providing the action (or `opendeckn3.builtin`).
    pub plugin: String,
    /// UUID of the action inside the plugin.
    pub action: String,
    /// Arbitrary per-instance settings, owned by the plugin.
    #[serde(default = "empty_object")]
    pub settings: Value,
    #[serde(default)]
    pub state: u16,
    #[serde(default)]
    pub title: Option<String>,
    /// User-chosen image override as data URL.
    #[serde(default)]
    pub image: Option<String>,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

impl ActionInstance {
    pub fn new(plugin: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            action: action.into(),
            settings: empty_object(),
            state: 0,
            title: None,
            image: None,
        }
    }
}

/// A page of bindings for one device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub keys: BTreeMap<u8, ActionInstance>,
    #[serde(default)]
    pub encoders: BTreeMap<u8, ActionInstance>,
}

impl Profile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn slots(&self, controller: Controller) -> &BTreeMap<u8, ActionInstance> {
        match controller {
            Controller::Keypad => &self.keys,
            Controller::Encoder => &self.encoders,
        }
    }

    pub fn slots_mut(&mut self, controller: Controller) -> &mut BTreeMap<u8, ActionInstance> {
        match controller {
            Controller::Keypad => &mut self.keys,
            Controller::Encoder => &mut self.encoders,
        }
    }
}

/// Uniquely identifies a slot; serialized as the Stream Deck SDK `context`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlotContext {
    pub device: String,
    pub profile: String,
    pub controller: Controller,
    pub position: u8,
}

impl SlotContext {
    pub fn encode(&self) -> String {
        let controller = match self.controller {
            Controller::Keypad => "key",
            Controller::Encoder => "enc",
        };
        format!(
            "{}|{}|{}|{}",
            self.device, self.profile, controller, self.position
        )
    }

    pub fn decode(context: &str) -> Option<Self> {
        let mut parts = context.rsplitn(4, '|');
        let position = parts.next()?.parse().ok()?;
        let controller = match parts.next()? {
            "key" => Controller::Keypad,
            "enc" => Controller::Encoder,
            _ => return None,
        };
        let profile = parts.next()?.to_owned();
        let device = parts.next()?.to_owned();
        Some(Self {
            device,
            profile,
            controller,
            position,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_roundtrip() {
        let ctx = SlotContext {
            device: "n3-ABC".into(),
            profile: "default".into(),
            controller: Controller::Encoder,
            position: 2,
        };
        assert_eq!(SlotContext::decode(&ctx.encode()), Some(ctx));
        assert_eq!(SlotContext::decode("garbage"), None);
    }

    #[test]
    fn profile_json_uses_string_keys() {
        let mut profile = Profile::new("default", "Default");
        profile
            .keys
            .insert(3, ActionInstance::new("com.example", "com.example.hello"));
        let json = serde_json::to_string(&profile).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, profile);
    }
}

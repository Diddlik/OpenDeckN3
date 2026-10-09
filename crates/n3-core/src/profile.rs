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

/// One page of bindings: what the keys and encoders do while it is shown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// Optional display name; empty = "Seite <n>".
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub keys: BTreeMap<u8, ActionInstance>,
    #[serde(default)]
    pub encoders: BTreeMap<u8, ActionInstance>,
}

impl Page {
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

/// A set of pages for one device. Always has at least one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredProfile")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub pages: Vec<Page>,
}

/// On-disk form; profiles from before pages existed kept `keys`/`encoders`
/// at the top level, which become the first page.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProfile {
    id: String,
    name: String,
    #[serde(default)]
    pages: Vec<Page>,
    #[serde(default)]
    keys: BTreeMap<u8, ActionInstance>,
    #[serde(default)]
    encoders: BTreeMap<u8, ActionInstance>,
}

impl From<StoredProfile> for Profile {
    fn from(stored: StoredProfile) -> Self {
        let mut pages = stored.pages;
        if pages.is_empty() {
            pages.push(Page {
                name: String::new(),
                keys: stored.keys,
                encoders: stored.encoders,
            });
        }
        Self {
            id: stored.id,
            name: stored.name,
            pages,
        }
    }
}

impl Profile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            pages: vec![Page::default()],
        }
    }

    /// Page `index`, falling back to the first page.
    pub fn page(&self, index: usize) -> &Page {
        self.pages.get(index).unwrap_or(&self.pages[0])
    }

    pub fn page_mut(&mut self, index: usize) -> &mut Page {
        let index = if index < self.pages.len() { index } else { 0 };
        &mut self.pages[index]
    }
}

/// Uniquely identifies a slot; serialized as the Stream Deck SDK `context`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlotContext {
    pub device: String,
    pub profile: String,
    pub page: u8,
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
            "{}|{}|{}|{}|{}",
            self.device, self.profile, self.page, controller, self.position
        )
    }

    pub fn decode(context: &str) -> Option<Self> {
        let mut parts = context.rsplitn(5, '|');
        let position = parts.next()?.parse().ok()?;
        let controller = match parts.next()? {
            "key" => Controller::Keypad,
            "enc" => Controller::Encoder,
            _ => return None,
        };
        let page = parts.next()?.parse().ok()?;
        let profile = parts.next()?.to_owned();
        let device = parts.next()?.to_owned();
        Some(Self {
            device,
            profile,
            page,
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
            page: 1,
            controller: Controller::Encoder,
            position: 2,
        };
        assert_eq!(SlotContext::decode(&ctx.encode()), Some(ctx));
        assert_eq!(SlotContext::decode("garbage"), None);
    }

    #[test]
    fn profile_json_uses_string_keys() {
        let mut profile = Profile::new("default", "Default");
        profile.pages[0]
            .keys
            .insert(3, ActionInstance::new("com.example", "com.example.hello"));
        profile.pages.push(Page::default());
        let json = serde_json::to_string(&profile).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, profile);
    }

    #[test]
    fn old_profiles_become_the_first_page() {
        let old = r#"{"id":"default","name":"default","keys":{"2":{"plugin":"p","action":"p.a"}},"encoders":{}}"#;
        let profile: Profile = serde_json::from_str(old).unwrap();
        assert_eq!(profile.pages.len(), 1);
        assert_eq!(profile.pages[0].keys[&2].action, "p.a");
        assert_eq!(
            profile.page(7).keys.len(),
            1,
            "out of range falls back to page 1"
        );
    }
}

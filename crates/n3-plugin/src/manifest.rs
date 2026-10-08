//! `manifest.json` of a plugin (Stream Deck SDK format, PascalCase keys).

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PluginManifest {
    /// Explicit plugin UUID (SDK v2). Falls back to the directory name.
    #[serde(rename = "UUID", default)]
    pub uuid: Option<String>,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub actions: Vec<ActionManifest>,
    #[serde(default)]
    pub code_path: Option<String>,
    #[serde(default)]
    pub code_path_lin: Option<String>,
    #[serde(default)]
    pub code_path_mac: Option<String>,
    #[serde(default)]
    pub code_path_win: Option<String>,
    #[serde(default)]
    pub property_inspector_path: Option<String>,
    /// OpenDeckN3 extension: form for the plugin's global settings
    /// (same field format as `SettingsSchema`).
    #[serde(default)]
    pub global_settings_schema: Option<serde_json::Value>,
}

fn default_category() -> String {
    "Custom".to_owned()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActionManifest {
    #[serde(rename = "UUID")]
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub property_inspector_path: Option<String>,
    /// `Keypad` and/or `Encoder`. Defaults to `Keypad`.
    #[serde(default = "default_controllers")]
    pub controllers: Vec<String>,
    #[serde(default)]
    pub states: Vec<ActionState>,
    /// OpenDeckN3 extension: settings form shown in the app instead of a
    /// property inspector (see docs/PLUGIN_API.md).
    #[serde(default)]
    pub settings_schema: Option<serde_json::Value>,
}

fn default_controllers() -> Vec<String> {
    vec!["Keypad".to_owned()]
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActionState {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub title: Option<String>,
}

impl PluginManifest {
    pub fn read(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("manifest.json");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .with_context(|| format!("parsing {}", path.display()))
    }

    /// Executable (relative to the plugin dir) for the current platform.
    pub fn code_path_for_current_os(&self) -> Option<&str> {
        let specific = match std::env::consts::OS {
            "linux" => self.code_path_lin.as_deref(),
            "macos" => self.code_path_mac.as_deref(),
            "windows" => self.code_path_win.as_deref(),
            _ => None,
        };
        specific.or(self.code_path.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stream_deck_manifest() {
        let manifest: PluginManifest = serde_json::from_str(
            r#"{
                "Name": "Hello",
                "Author": "me",
                "Version": "1.0.0",
                "CodePath": "plugin.js",
                "Actions": [
                    { "UUID": "com.example.hello.press", "Name": "Press", "Controllers": ["Keypad", "Encoder"] },
                    { "UUID": "com.example.hello.other", "Name": "Other" }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.actions.len(), 2);
        assert_eq!(manifest.actions[1].controllers, vec!["Keypad"]);
        assert_eq!(manifest.code_path_for_current_os(), Some("plugin.js"));
        assert_eq!(manifest.category, "Custom");
    }
}

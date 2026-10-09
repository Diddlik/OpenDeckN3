//! JSON persistence below the config directory.
//!
//! ```text
//! <config>/devices/<device-id>/device.json
//! <config>/devices/<device-id>/profiles/<profile-id>.json
//! <config>/plugin-settings/<plugin-uuid>.json
//! <config>/settings.json
//! ```

use std::path::{Path, PathBuf};

use anyhow::Context;
use n3_core::Profile;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

pub const DEFAULT_PROFILE: &str = "default";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    pub active_profile: String,
    pub brightness: u8,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            active_profile: DEFAULT_PROFILE.to_owned(),
            brightness: 50,
        }
    }
}

/// Settings of the service itself (not per device).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// Install newer plugin versions from the catalog automatically.
    pub auto_update_plugins: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_update_plugins: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

/// Rejects ids that could escape the config directory.
fn safe_id(id: &str) -> anyhow::Result<&str> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !id.starts_with('.');
    anyhow::ensure!(ok, "invalid id {id:?}");
    Ok(id)
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn device_dir(&self, device: &str) -> anyhow::Result<PathBuf> {
        Ok(self.root.join("devices").join(safe_id(device)?))
    }

    fn profile_path(&self, device: &str, profile: &str) -> anyhow::Result<PathBuf> {
        Ok(self
            .device_dir(device)?
            .join("profiles")
            .join(format!("{}.json", safe_id(profile)?)))
    }

    pub fn load_device_config(&self, device: &str) -> anyhow::Result<DeviceConfig> {
        Ok(read_json(&self.device_dir(device)?.join("device.json"))?.unwrap_or_default())
    }

    pub fn save_device_config(&self, device: &str, config: &DeviceConfig) -> anyhow::Result<()> {
        write_json(&self.device_dir(device)?.join("device.json"), config)
    }

    /// Loads a profile, creating an empty one if it does not exist yet.
    pub fn load_profile(&self, device: &str, profile: &str) -> anyhow::Result<Profile> {
        let path = self.profile_path(device, profile)?;
        Ok(read_json(&path)?.unwrap_or_else(|| Profile::new(profile, profile)))
    }

    pub fn save_profile(&self, device: &str, profile: &Profile) -> anyhow::Result<()> {
        write_json(&self.profile_path(device, &profile.id)?, profile)
    }

    pub fn delete_profile(&self, device: &str, profile: &str) -> anyhow::Result<()> {
        let path = self.profile_path(device, profile)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn list_profiles(&self, device: &str) -> anyhow::Result<Vec<String>> {
        let dir = self.device_dir(device)?.join("profiles");
        let mut ids = Vec::new();
        if dir.exists() {
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "json")
                    && let Some(stem) = path.file_stem()
                {
                    ids.push(stem.to_string_lossy().into_owned());
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn load_app_settings(&self) -> anyhow::Result<AppSettings> {
        Ok(read_json(&self.root.join("settings.json"))?.unwrap_or_default())
    }

    pub fn save_app_settings(&self, settings: &AppSettings) -> anyhow::Result<()> {
        write_json(&self.root.join("settings.json"), settings)
    }

    pub fn load_global_settings(&self, plugin: &str) -> anyhow::Result<Value> {
        let path = self
            .root
            .join("plugin-settings")
            .join(format!("{}.json", safe_id(plugin)?));
        Ok(read_json(&path)?.unwrap_or_else(|| Value::Object(Default::default())))
    }

    pub fn save_global_settings(&self, plugin: &str, settings: &Value) -> anyhow::Result<()> {
        let path = self
            .root
            .join("plugin-settings")
            .join(format!("{}.json", safe_id(plugin)?));
        write_json(&path, settings)
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write-then-rename so a crash never leaves a truncated file behind.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use n3_core::ActionInstance;

    use super::*;

    #[test]
    fn profile_roundtrip_and_listing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());

        let mut profile = store.load_profile("n3-1", "work").unwrap();
        assert!(profile.pages[0].keys.is_empty());
        profile.pages[0]
            .keys
            .insert(0, ActionInstance::new("p", "p.a"));
        store.save_profile("n3-1", &profile).unwrap();

        assert_eq!(store.load_profile("n3-1", "work").unwrap(), profile);
        assert_eq!(store.list_profiles("n3-1").unwrap(), vec!["work"]);
        store.delete_profile("n3-1", "work").unwrap();
        assert!(store.list_profiles("n3-1").unwrap().is_empty());
    }

    #[test]
    fn app_settings_default_and_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(store.load_app_settings().unwrap().auto_update_plugins);
        store
            .save_app_settings(&AppSettings {
                auto_update_plugins: false,
            })
            .unwrap();
        assert!(!store.load_app_settings().unwrap().auto_update_plugins);
    }

    #[test]
    fn rejects_path_traversal() {
        let store = Store::new("/tmp/none");
        assert!(store.load_profile("n3-1", "../evil").is_err());
        assert!(store.load_device_config("..").is_err());
    }
}

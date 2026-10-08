use std::sync::Arc;

use image::DynamicImage;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Physical layout of a device as exposed to profiles, plugins and the UI.
///
/// Keys are numbered row-major starting at 0. The first `display_keys` keys
/// have an LCD, the remaining ones are plain buttons.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLayout {
    pub rows: u8,
    pub columns: u8,
    pub keys: u8,
    pub display_keys: u8,
    pub encoders: u8,
    /// Native key image size in pixels (width, height) before rotation.
    pub key_image_size: (u32, u32),
}

impl DeviceLayout {
    /// Row/column of a key, used for Stream Deck SDK `coordinates`.
    pub fn key_coordinates(&self, key: u8) -> Coordinates {
        Coordinates {
            row: key / self.columns,
            column: key % self.columns,
        }
    }

    pub fn has_display(&self, key: u8) -> bool {
        key < self.display_keys
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coordinates {
    pub column: u8,
    pub row: u8,
}

/// Static information about a connected device.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Stable id, `<namespace>-<serial>`, e.g. `n3-355499441494`.
    pub id: String,
    /// Human readable model name, e.g. `TreasLin N3`.
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub firmware: Option<String>,
    /// `true` for the software simulator.
    pub virtual_device: bool,
    pub layout: DeviceLayout,
}

/// Which kind of control an input or binding refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Controller {
    Keypad,
    Encoder,
}

/// Normalized input coming from any device driver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum InputEvent {
    KeyDown { key: u8 },
    KeyUp { key: u8 },
    EncoderDown { encoder: u8 },
    EncoderUp { encoder: u8 },
    EncoderTwist { encoder: u8, ticks: i16 },
}

/// Commands the daemon sends to a device driver.
#[derive(Clone, Debug)]
pub enum DeviceCommand {
    /// Set (`Some`) or clear (`None`) the image of a display key.
    SetKeyImage {
        key: u8,
        image: Option<Arc<DynamicImage>>,
    },
    ClearAll,
    /// Display brightness, 0-100.
    SetBrightness(u8),
}

/// Events emitted by drivers towards the daemon.
#[derive(Debug)]
pub enum DeviceEvent {
    Connected(DeviceHandle),
    Disconnected { device: String },
    Input { device: String, event: InputEvent },
}

/// Handle to a running device task. Cheap to clone.
#[derive(Clone, Debug)]
pub struct DeviceHandle {
    pub info: DeviceInfo,
    commands: mpsc::Sender<DeviceCommand>,
}

impl DeviceHandle {
    pub fn new(info: DeviceInfo, commands: mpsc::Sender<DeviceCommand>) -> Self {
        Self { info, commands }
    }

    /// Queues a command for the device. Returns `false` if the device is gone.
    pub async fn send(&self, command: DeviceCommand) -> bool {
        self.commands.send(command).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_row_major() {
        let layout = DeviceLayout {
            rows: 3,
            columns: 3,
            keys: 9,
            display_keys: 6,
            encoders: 3,
            key_image_size: (64, 64),
        };
        assert_eq!(layout.key_coordinates(0), Coordinates { column: 0, row: 0 });
        assert_eq!(layout.key_coordinates(4), Coordinates { column: 1, row: 1 });
        assert_eq!(layout.key_coordinates(8), Coordinates { column: 2, row: 2 });
        assert!(layout.has_display(5));
        assert!(!layout.has_display(6));
    }
}

//! Table of supported hardware models.

use mirajazz::{
    device::DeviceQuery,
    error::MirajazzError,
    types::{DeviceInput, ImageFormat, ImageMirroring, ImageMode, ImageRotation},
};
use n3_core::DeviceLayout;

/// Signature mirajazz expects for decoding raw input reports.
pub type ProcessInputFn = fn(u8, u8) -> Result<DeviceInput, MirajazzError>;

/// HID usage page/usage the Mirabox N3 family exposes its control interface on.
const USAGE_PAGE: u16 = 65440;
const USAGE_ID: u16 = 1;

/// Static description of a supported device model.
#[derive(Debug)]
pub struct ModelSpec {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Short prefix for device ids, e.g. `n3` → `n3-<serial>`.
    pub namespace: &'static str,
    /// Mirajazz protocol version (1-3).
    pub protocol_version: usize,
    pub image_format: ImageFormat,
    pub layout: DeviceLayout,
    /// Maps raw `(input, state)` bytes to a normalized mirajazz input.
    pub process_input: ProcessInputFn,
}

impl ModelSpec {
    pub const fn query(&self) -> DeviceQuery {
        DeviceQuery::new(USAGE_PAGE, USAGE_ID, self.vendor_id, self.product_id)
    }
}

/// Layout shared by the whole N3 family: 6 LCD keys (3×2), 3 plain buttons
/// below and 3 rotary encoders with push function.
pub const N3_LAYOUT: DeviceLayout = DeviceLayout {
    rows: 3,
    columns: 3,
    keys: 9,
    display_keys: 6,
    encoders: 3,
    key_image_size: (64, 64),
};

/// TreasLin N3 (VID 0x5548, PID 0x1001). Protocol v3, 64×64 JPEG rotated by 90°.
pub const TREASLIN_N3: ModelSpec = ModelSpec {
    name: "TreasLin N3",
    vendor_id: 0x5548,
    product_id: 0x1001,
    namespace: "n3",
    protocol_version: 3,
    image_format: ImageFormat {
        mode: ImageMode::JPEG,
        size: (64, 64),
        rotation: ImageRotation::Rot90,
        mirror: ImageMirroring::None,
    },
    layout: N3_LAYOUT,
    process_input: crate::n3::process_input,
};

/// All models the HID watcher looks for. Extend this list to add devices.
pub const SUPPORTED_MODELS: &[&ModelSpec] = &[&TREASLIN_N3];

pub fn find_model(vendor_id: u16, product_id: u16) -> Option<&'static ModelSpec> {
    SUPPORTED_MODELS
        .iter()
        .copied()
        .find(|m| m.vendor_id == vendor_id && m.product_id == product_id)
}

pub fn queries() -> Vec<DeviceQuery> {
    SUPPORTED_MODELS.iter().map(|m| m.query()).collect()
}

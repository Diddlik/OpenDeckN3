//! Software-only N3 for development without hardware (UI work, plugin tests).
//!
//! The virtual device accepts all commands and logs them. Inputs are injected
//! by the daemon (e.g. via the UI API's `simulateInput`).

use n3_core::{DeviceCommand, DeviceEvent, DeviceHandle, DeviceInfo};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::models::TREASLIN_N3;

pub const VIRTUAL_DEVICE_ID: &str = "virtual-n3";

pub async fn run_virtual_device(events: mpsc::Sender<DeviceEvent>, token: CancellationToken) {
    let info = DeviceInfo {
        id: VIRTUAL_DEVICE_ID.to_owned(),
        name: format!("{} (virtuell)", TREASLIN_N3.name),
        vendor_id: TREASLIN_N3.vendor_id,
        product_id: TREASLIN_N3.product_id,
        serial: "VIRTUAL".to_owned(),
        firmware: None,
        virtual_device: true,
        layout: TREASLIN_N3.layout.clone(),
    };

    let (command_tx, mut command_rx) = mpsc::channel(64);
    if events
        .send(DeviceEvent::Connected(DeviceHandle::new(info, command_tx)))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            command = command_rx.recv() => match command {
                Some(DeviceCommand::SetKeyImage { key, image }) => {
                    tracing::debug!(key, has_image = image.is_some(), "virtual: set key image");
                }
                Some(other) => tracing::debug!(?other, "virtual: command"),
                None => break,
            },
            _ = token.cancelled() => break,
        }
    }

    events
        .send(DeviceEvent::Disconnected {
            device: VIRTUAL_DEVICE_ID.to_owned(),
        })
        .await
        .ok();
}

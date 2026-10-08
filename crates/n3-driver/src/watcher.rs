//! Hot-plug detection for HID devices.

use std::collections::HashMap;

use futures_lite::StreamExt;
use mirajazz::{
    device::{DeviceWatcher, list_devices},
    error::MirajazzError,
    types::{DeviceLifecycleEvent, HidDeviceInfo},
};
use n3_core::DeviceEvent;
use tokio::sync::mpsc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::models::{self, ModelSpec};

fn device_id(model: &ModelSpec, dev: &HidDeviceInfo) -> Option<String> {
    Some(format!(
        "{}-{}",
        model.namespace,
        dev.serial_number.as_ref()?
    ))
}

/// Scans for supported devices, then watches for hot-plug events until
/// `token` is cancelled. Each device gets its own task.
pub async fn run_hid_watcher(
    events: mpsc::Sender<DeviceEvent>,
    token: CancellationToken,
) -> Result<(), MirajazzError> {
    let queries = models::queries();
    let tracker = TaskTracker::new();
    let mut running: HashMap<String, CancellationToken> = HashMap::new();

    let spawn = |dev: HidDeviceInfo, running: &mut HashMap<String, CancellationToken>| {
        let Some(model) = models::find_model(dev.vendor_id, dev.product_id) else {
            return;
        };
        let Some(id) = device_id(model, &dev) else {
            tracing::warn!(?dev, "device without serial number ignored");
            return;
        };
        if running.get(&id).is_some_and(|t| !t.is_cancelled()) {
            return;
        }
        let child = token.child_token();
        running.insert(id.clone(), child.clone());
        tracker.spawn(crate::n3::run_device(model, id, dev, events.clone(), child));
    };

    for dev in list_devices(&queries).await? {
        spawn((*dev).clone(), &mut running);
    }

    let mut watcher = DeviceWatcher::new();
    let mut stream = watcher.watch(&queries).await?;
    tracing::info!("HID watcher ready");

    loop {
        let event = tokio::select! {
            ev = stream.next() => ev,
            _ = token.cancelled() => None,
        };
        match event {
            Some(DeviceLifecycleEvent::Connected(dev)) => spawn(dev, &mut running),
            Some(DeviceLifecycleEvent::Disconnected(dev)) => {
                let model = models::find_model(dev.vendor_id, dev.product_id);
                if let Some(id) = model.and_then(|m| device_id(m, &dev))
                    && let Some(token) = running.remove(&id)
                {
                    token.cancel();
                }
            }
            None => break,
        }
    }

    tracker.close();
    tracker.wait().await;
    Ok(())
}

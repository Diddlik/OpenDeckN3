//! Mirabox N3 family protocol glue (TreasLin N3 and relatives).
//!
//! Input mapping is taken from the `opendeck-akp03` plugin
//! (https://github.com/4ndv/opendeck-akp03, GPL-3.0).

use std::{sync::Arc, time::Duration};

use mirajazz::{
    device::Device,
    error::MirajazzError,
    state::DeviceStateUpdate,
    types::{DeviceInput, HidDeviceInfo},
};
use n3_core::{DeviceCommand, DeviceEvent, DeviceHandle, DeviceInfo, InputEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::models::{ModelSpec, N3_LAYOUT};

const KEY_COUNT: usize = N3_LAYOUT.keys as usize;
const ENCODER_COUNT: usize = N3_LAYOUT.encoders as usize;
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
const DEFAULT_BRIGHTNESS: u8 = 50;
/// Pause before retrying a device that could not be opened (e.g. it is in
/// use by the vendor software), so the rescan does not hammer it.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10);

/// Decodes a raw N3 input report `(input, state)` into a mirajazz input.
///
/// Raw codes:
/// - `0x01..=0x06`: LCD keys → key 0..5
/// - `0x25`, `0x30`, `0x31`: plain buttons → key 6..8
/// - `0x90/0x91`, `0x50/0x51`, `0x60/0x61`: encoder 0/1/2 twist left/right
/// - `0x33`, `0x35`, `0x34`: encoder 0/1/2 press
pub fn process_input(input: u8, state: u8) -> Result<DeviceInput, MirajazzError> {
    tracing::trace!(input, state, "raw N3 input");

    let mut keys = vec![false; KEY_COUNT];
    let key = match input {
        0 => return Ok(DeviceInput::ButtonStateChange(keys)),
        1..=6 => Some(input as usize - 1),
        0x25 => Some(6),
        0x30 => Some(7),
        0x31 => Some(8),
        _ => None,
    };
    if let Some(key) = key {
        keys[key] = state != 0;
        return Ok(DeviceInput::ButtonStateChange(keys));
    }

    let twist = match input {
        0x90 => Some((0, -1)),
        0x91 => Some((0, 1)),
        0x50 => Some((1, -1)),
        0x51 => Some((1, 1)),
        0x60 => Some((2, -1)),
        0x61 => Some((2, 1)),
        _ => None,
    };
    if let Some((encoder, ticks)) = twist {
        let mut values = vec![0i8; ENCODER_COUNT];
        values[encoder] = ticks;
        return Ok(DeviceInput::EncoderTwist(values));
    }

    let press = match input {
        0x33 => Some(0),
        0x35 => Some(1),
        0x34 => Some(2),
        _ => None,
    };
    if let Some(encoder) = press {
        let mut states = vec![false; ENCODER_COUNT];
        states[encoder] = state != 0;
        return Ok(DeviceInput::EncoderStateChange(states));
    }

    Err(MirajazzError::BadData)
}

fn to_input_event(update: DeviceStateUpdate) -> InputEvent {
    match update {
        DeviceStateUpdate::ButtonDown(key) => InputEvent::KeyDown { key },
        DeviceStateUpdate::ButtonUp(key) => InputEvent::KeyUp { key },
        DeviceStateUpdate::EncoderDown(encoder) => InputEvent::EncoderDown { encoder },
        DeviceStateUpdate::EncoderUp(encoder) => InputEvent::EncoderUp { encoder },
        DeviceStateUpdate::EncoderTwist(encoder, ticks) => InputEvent::EncoderTwist {
            encoder,
            ticks: ticks as i16,
        },
    }
}

/// Runs one physical device on its own OS thread until it disconnects or
/// `token` is cancelled, then cancels `token` so the watcher may reconnect it.
///
/// Windows cancels pending overlapped I/O (`ERROR_OPERATION_ABORTED`) when
/// the thread that issued it exits. On a shared runtime the HID read can be
/// issued from a thread that later goes away; a dedicated thread that lives
/// as long as the connection rules that out.
pub async fn run_device(
    model: &'static ModelSpec,
    id: String,
    dev: HidDeviceInfo,
    events: mpsc::Sender<DeviceEvent>,
    token: CancellationToken,
) {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<bool>();
    let thread_token = token.clone();
    let thread_id = id.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("hid-{id}"))
        .spawn(move || {
            let runtimes = (
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build(),
                // mirajazz converts key images inside `block_in_place`, which panics on a
                // current-thread runtime. Conversion only fills the image cache; the HID
                // writes (`flush`) stay on this thread.
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("n3-images")
                    .build(),
            );
            let connected = match runtimes {
                (Ok(rt), Ok(images)) => rt.block_on(device_session(
                    model,
                    thread_id,
                    dev,
                    events,
                    thread_token,
                    images.handle().clone(),
                )),
                (Err(err), _) | (_, Err(err)) => {
                    tracing::error!(%err, "cannot start device runtime");
                    false
                }
            };
            done_tx.send(connected).ok();
        });
    let connected = match spawned {
        Ok(_) => done_rx.await.unwrap_or_else(|_| {
            // Panics go to stderr, which the desktop app does not show.
            tracing::error!(%id, "device thread panicked");
            false
        }),
        Err(err) => {
            tracing::error!(%err, "cannot start device thread");
            false
        }
    };
    if !connected {
        tokio::select! {
            _ = tokio::time::sleep(RETRY_AFTER_FAILURE) => {}
            _ = token.cancelled() => {}
        }
    }
    token.cancel();
}

/// Returns `false` if the device could not be opened.
async fn device_session(
    model: &'static ModelSpec,
    id: String,
    dev: HidDeviceInfo,
    events: mpsc::Sender<DeviceEvent>,
    token: CancellationToken,
    images: tokio::runtime::Handle,
) -> bool {
    let device = match connect(model, &dev).await {
        Ok(device) => Arc::new(device),
        Err(err) => {
            tracing::error!(%id, %err, "failed to initialise device");
            return false;
        }
    };

    let info = DeviceInfo {
        id: id.clone(),
        name: model.name.to_owned(),
        vendor_id: model.vendor_id,
        product_id: model.product_id,
        serial: device.serial_number.clone(),
        firmware: device.firmware_version.clone(),
        virtual_device: false,
        layout: model.layout.clone(),
    };
    tracing::info!(?info, "device connected");

    let (command_tx, command_rx) = mpsc::channel(64);
    if events
        .send(DeviceEvent::Connected(DeviceHandle::new(info, command_tx)))
        .await
        .is_err()
    {
        return true;
    }

    tokio::select! {
        res = read_loop(&id, &device, model, &events) => {
            if let Err(err) = res {
                tracing::warn!(%id, %err, "read loop ended");
            }
        }
        res = command_loop(&device, model, command_rx, &images) => {
            if let Err(err) = res {
                tracing::warn!(%id, %err, "command loop ended");
            }
        }
        _ = token.cancelled() => {}
    }

    device.shutdown().await.ok();
    events
        .send(DeviceEvent::Disconnected { device: id.clone() })
        .await
        .ok();
    tracing::info!(%id, "device task finished");
    true
}

async fn connect(model: &ModelSpec, dev: &HidDeviceInfo) -> Result<Device, MirajazzError> {
    let device = Device::connect(dev, model.protocol_version, KEY_COUNT, ENCODER_COUNT).await?;
    device.set_brightness(DEFAULT_BRIGHTNESS).await?;
    device.clear_all_button_images().await?;
    device.flush().await?;
    Ok(device)
}

async fn read_loop(
    id: &str,
    device: &Device,
    model: &ModelSpec,
    events: &mpsc::Sender<DeviceEvent>,
) -> Result<(), MirajazzError> {
    let reader = device.get_reader(model.process_input);
    loop {
        let updates = match reader.read(None).await {
            Ok(updates) => updates,
            // Unknown input codes are not fatal.
            Err(MirajazzError::BadData) => continue,
            Err(err) => return Err(err),
        };
        for update in updates {
            let event = DeviceEvent::Input {
                device: id.to_owned(),
                event: to_input_event(update),
            };
            if events.send(event).await.is_err() {
                return Ok(());
            }
        }
    }
}

async fn command_loop(
    device: &Arc<Device>,
    model: &ModelSpec,
    mut commands: mpsc::Receiver<DeviceCommand>,
    images: &tokio::runtime::Handle,
) -> Result<(), MirajazzError> {
    let mut keep_alive = tokio::time::interval(KEEP_ALIVE_INTERVAL);
    loop {
        tokio::select! {
            _ = keep_alive.tick() => device.keep_alive().await?,
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()) };
                match apply_command(device, model, command, images).await {
                    Ok(()) => {}
                    // A broken image must not take the device down.
                    Err(MirajazzError::ImageError(err)) => tracing::warn!(%err, "image conversion failed"),
                    Err(err) => return Err(err),
                }
            }
        }
    }
}

async fn apply_command(
    device: &Arc<Device>,
    model: &ModelSpec,
    command: DeviceCommand,
    images: &tokio::runtime::Handle,
) -> Result<(), MirajazzError> {
    match command {
        DeviceCommand::SetKeyImage { key, image } => {
            if !model.layout.has_display(key) {
                return Ok(());
            }
            match image {
                Some(image) => {
                    let (device, format) = (device.clone(), model.image_format);
                    let converted = images
                        .spawn(async move {
                            device.set_button_image(key, format, (*image).clone()).await
                        })
                        .await;
                    match converted {
                        Ok(result) => result?,
                        Err(err) => tracing::warn!(%err, "image conversion failed"),
                    }
                }
                None => device.clear_button_image(key).await?,
            }
        }
        DeviceCommand::ClearAll => device.clear_all_button_images().await?,
        DeviceCommand::SetBrightness(percent) => device.set_brightness(percent).await?,
    }
    device.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The device thread runs a current-thread runtime, where mirajazz's image
    /// conversion panics; it has to go through the separate image runtime.
    #[test]
    fn key_images_convert_off_the_device_runtime() {
        let format = crate::models::TREASLIN_N3.image_format;
        let image = || image::DynamicImage::new_rgb8(64, 64);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let direct = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.block_on(mirajazz::images::convert_image_with_format(format, image()))
        }));
        assert!(direct.is_err(), "mirajazz no longer needs block_in_place");

        let images = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        let handle = images.handle().clone();
        let data = rt
            .block_on(handle.spawn(mirajazz::images::convert_image_with_format(format, image())))
            .unwrap()
            .unwrap();
        assert!(!data.is_empty());
    }

    fn pressed_key(input: DeviceInput) -> Option<usize> {
        match input {
            DeviceInput::ButtonStateChange(keys) => keys.iter().position(|k| *k),
            _ => panic!("expected button change"),
        }
    }

    #[test]
    fn maps_display_and_plain_keys() {
        assert_eq!(pressed_key(process_input(0x01, 1).unwrap()), Some(0));
        assert_eq!(pressed_key(process_input(0x06, 1).unwrap()), Some(5));
        assert_eq!(pressed_key(process_input(0x25, 1).unwrap()), Some(6));
        assert_eq!(pressed_key(process_input(0x31, 1).unwrap()), Some(8));
        assert_eq!(pressed_key(process_input(0x31, 0).unwrap()), None);
    }

    #[test]
    fn maps_encoders() {
        match process_input(0x51, 0).unwrap() {
            DeviceInput::EncoderTwist(v) => assert_eq!(v, vec![0, 1, 0]),
            other => panic!("unexpected {other:?}"),
        }
        match process_input(0x34, 1).unwrap() {
            DeviceInput::EncoderStateChange(v) => assert_eq!(v, vec![false, false, true]),
            other => panic!("unexpected {other:?}"),
        }
        assert!(process_input(0x77, 1).is_err());
    }
}

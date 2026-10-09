//! The daemon's central state and event router.
//!
//! All state lives in [`App`] and is only touched from the main loop in
//! `main.rs`; drivers, plugins and UI clients talk to it via channels. This
//! keeps the logic single-threaded and free of locks.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, anyhow, bail};
use image::DynamicImage;
use n3_core::{
    ActionInstance, Controller, Coordinates, DeviceCommand, DeviceEvent, DeviceHandle, InputEvent,
    Page, Profile, SlotContext,
};
use n3_plugin::{
    BUILTIN_PLUGIN, InboundEvent, PluginEvent, PluginHost, PluginMessage, protocol,
    protocol::SlotRef,
};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    api::ApiCommand,
    builtin, render,
    store::{DeviceConfig, Store},
    system::input::InputHandle,
};

pub struct DeviceState {
    pub handle: DeviceHandle,
    pub config: DeviceConfig,
    pub profile: Profile,
    /// Index of the page shown (not persisted: a profile always opens on page 1).
    pub page: usize,
    /// Keys held down right now, and keys whose release must not reach the
    /// action that appeared under the finger after a page/profile switch.
    held_keys: BTreeSet<u8>,
    swallow_release: BTreeSet<u8>,
    /// Image currently shown per key (PNG data URL), mirrored for the UI.
    pub previews: BTreeMap<u8, String>,
    /// Titles set at runtime by plugins (not persisted, like Stream Deck).
    pub titles: BTreeMap<u8, String>,
    /// Images set at runtime by plugins (`setImage`), cleared on profile switch.
    pub runtime_images: BTreeMap<u8, DynamicImage>,
    pub encoders_pressed: Vec<bool>,
}

impl DeviceState {
    pub fn page(&self) -> &Page {
        self.profile.page(self.page)
    }

    fn page_mut(&mut self) -> &mut Page {
        self.profile.page_mut(self.page)
    }

    fn coordinates(&self, controller: Controller, position: u8) -> Coordinates {
        match controller {
            Controller::Keypad => self.handle.info.layout.key_coordinates(position),
            Controller::Encoder => Coordinates {
                column: position,
                row: 0,
            },
        }
    }

    fn context(&self, controller: Controller, position: u8) -> String {
        SlotContext {
            device: self.handle.info.id.clone(),
            profile: self.profile.id.clone(),
            page: self.page as u8,
            controller,
            position,
        }
        .encode()
    }

    /// Builds a plugin event for a slot, returning `(plugin, event)`.
    fn slot_event(
        &self,
        controller: Controller,
        position: u8,
        build: impl FnOnce(&SlotRef) -> Value,
    ) -> Option<(String, Value)> {
        let instance = self.page().slots(controller).get(&position)?;
        let context = self.context(controller, position);
        let slot = SlotRef {
            action: &instance.action,
            context: &context,
            device: &self.handle.info.id,
            controller,
            coordinates: self.coordinates(controller, position),
            settings: &instance.settings,
            state: instance.state,
        };
        Some((instance.plugin.clone(), build(&slot)))
    }

    /// `(controller, position)` of every bound slot in the active profile.
    fn bound_slots(&self) -> Vec<(Controller, u8)> {
        [Controller::Keypad, Controller::Encoder]
            .into_iter()
            .flat_map(|c| self.page().slots(c).keys().map(move |p| (c, *p)))
            .collect()
    }
}

pub struct App {
    pub store: Store,
    pub plugins: PluginHost,
    pub devices: HashMap<String, DeviceState>,
    ui: broadcast::Sender<Value>,
    /// Lets the UI start the virtual device at runtime.
    device_events: mpsc::Sender<DeviceEvent>,
    shutdown: CancellationToken,
    /// Simulated keyboard input for built-in actions.
    input: InputHandle,
    /// Where installed plugins live (`<config>/plugins`, absolute).
    plugins_dir: PathBuf,
    /// Bundled/extra plugin directories (read-only for the app).
    extra_plugin_dirs: Vec<PathBuf>,
    /// Open `pluginRequest`s: id → (plugin, sent at, reply).
    plugin_requests: HashMap<u64, PendingRequest>,
    next_request: u64,
}

type PendingRequest = (
    String,
    std::time::Instant,
    tokio::sync::oneshot::Sender<Result<Value, String>>,
);

impl App {
    pub fn new(
        store: Store,
        plugins: PluginHost,
        ui: broadcast::Sender<Value>,
        device_events: mpsc::Sender<DeviceEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            store,
            plugins,
            devices: HashMap::new(),
            ui,
            device_events,
            shutdown,
            input: InputHandle::spawn(),
            plugins_dir: PathBuf::new(),
            extra_plugin_dirs: Vec::new(),
            plugin_requests: HashMap::new(),
            next_request: 1,
        }
    }

    /// Forwards a `pluginRequest`; the reply comes later via
    /// `sendToPropertyInspector` (see `on_plugin_event`).
    pub async fn plugin_request(
        &mut self,
        plugin: String,
        payload: Value,
        reply: tokio::sync::oneshot::Sender<Result<Value, String>>,
    ) {
        if plugin == BUILTIN_PLUGIN {
            // Built-in dropdowns are answered here (scanning runs off the main loop).
            tokio::spawn(async move {
                let result = match payload["request"].as_str() {
                    Some("apps") => {
                        tokio::task::spawn_blocking(crate::system::apps::installed_apps)
                            .await
                            .map(|apps| json!({ "options": apps }))
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("unbekannte Anfrage".into()),
                };
                reply.send(result).ok();
            });
            return;
        }
        // Forget requests a plugin never answered.
        self.plugin_requests
            .retain(|_, (_, at, _)| at.elapsed() < std::time::Duration::from_secs(30));
        if !self.plugins.is_connected(&plugin).await {
            reply.send(Err("Plugin läuft nicht".into())).ok();
            return;
        }
        let id = self.next_request;
        self.next_request += 1;
        let mut payload = match payload {
            Value::Object(map) => Value::Object(map),
            other => json!({ "request": other }),
        };
        payload["requestId"] = json!(id);
        self.plugin_requests
            .insert(id, (plugin.clone(), std::time::Instant::now(), reply));
        self.plugins
            .send(&plugin, &protocol::send_to_plugin(&plugin, &payload))
            .await;
    }

    pub fn set_plugin_dirs(&mut self, plugins_dir: PathBuf, extra: Vec<PathBuf>) {
        self.plugins_dir = plugins_dir;
        self.extra_plugin_dirs = extra;
    }

    pub fn input(&self) -> InputHandle {
        self.input.clone()
    }

    /// For events emitted from background tasks (e.g. failed built-in actions).
    pub fn ui_sender(&self) -> broadcast::Sender<Value> {
        self.ui.clone()
    }

    fn emit(&self, event: Value) {
        // No UI connected is fine.
        self.ui.send(event).ok();
    }

    fn device(&self, id: &str) -> anyhow::Result<&DeviceState> {
        self.devices
            .get(id)
            .ok_or_else(|| anyhow!("unknown device {id}"))
    }

    fn device_mut(&mut self, id: &str) -> anyhow::Result<&mut DeviceState> {
        self.devices
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown device {id}"))
    }

    async fn send_slot_event(
        &self,
        device: &str,
        controller: Controller,
        position: u8,
        build: impl FnOnce(&SlotRef) -> Value,
    ) {
        let Some(state) = self.devices.get(device) else {
            return;
        };
        if let Some((plugin, event)) = state.slot_event(controller, position, build)
            && plugin != BUILTIN_PLUGIN
        {
            self.plugins.send(&plugin, &event).await;
        }
    }

    // ---------------------------------------------------------------------
    // Devices
    // ---------------------------------------------------------------------

    pub async fn on_device_event(&mut self, event: DeviceEvent) {
        match event {
            DeviceEvent::Connected(handle) => {
                if let Err(err) = self.device_connected(handle).await {
                    tracing::error!(%err, "failed to set up device");
                }
            }
            DeviceEvent::Disconnected { device } => self.device_disconnected(&device).await,
            DeviceEvent::Input { device, event } => {
                self.emit(json!({ "event": "input", "device": device, "input": event }));
                if let Err(err) = self.on_input(&device, event).await {
                    tracing::warn!(%device, %err, "input handling failed");
                }
            }
        }
    }

    async fn device_connected(&mut self, handle: DeviceHandle) -> anyhow::Result<()> {
        let id = handle.info.id.clone();
        let config = self.store.load_device_config(&id)?;
        let profile = self.store.load_profile(&id, &config.active_profile)?;
        self.store.save_profile(&id, &profile)?;

        handle
            .send(DeviceCommand::SetBrightness(config.brightness))
            .await;
        let encoders = handle.info.layout.encoders as usize;
        let info = handle.info.clone();
        self.devices.insert(
            id.clone(),
            DeviceState {
                handle,
                config,
                profile,
                page: 0,
                held_keys: BTreeSet::new(),
                swallow_release: BTreeSet::new(),
                previews: BTreeMap::new(),
                titles: BTreeMap::new(),
                runtime_images: BTreeMap::new(),
                encoders_pressed: vec![false; encoders],
            },
        );

        self.plugins
            .broadcast(&protocol::device_did_connect(&info))
            .await;
        self.emit(json!({ "event": "deviceConnected", "device": self.device_snapshot(&id)? }));
        self.activate_profile(&id).await;
        Ok(())
    }

    async fn device_disconnected(&mut self, id: &str) {
        if !self.devices.contains_key(id) {
            return;
        }
        self.deactivate_profile(id).await;
        self.devices.remove(id);
        self.plugins
            .broadcast(&protocol::device_did_disconnect(id))
            .await;
        self.emit(json!({ "event": "deviceDisconnected", "device": id }));
    }

    /// Keypad position that holds the button assignment of a knob press:
    /// knobs follow the keys, i.e. on the N3 E0/E1/E2 press = keys 9/10/11.
    fn knob_press_key(&self, device: &str, encoder: u8) -> anyhow::Result<u8> {
        Ok(self.device(device)?.handle.info.layout.keys + encoder)
    }

    async fn on_input(&mut self, device: &str, input: InputEvent) -> anyhow::Result<()> {
        // Pressing a knob: a button assignment ("Tasten" mode) wins; otherwise
        // plugins assigned to the knob get dialDown/dialUp (Stream Deck dials
        // expect that). Built-in rotation actions ignore presses.
        let knob_press = match input {
            InputEvent::EncoderDown { encoder } => Some((encoder, true)),
            InputEvent::EncoderUp { encoder } => Some((encoder, false)),
            _ => None,
        };
        let input = match knob_press {
            Some((encoder, down)) => {
                self.set_encoder_pressed(device, encoder, down)?;
                let key = self.knob_press_key(device, encoder)?;
                let has_button = self.device(device)?.page().keys.contains_key(&key);
                match (has_button, down) {
                    (true, true) => InputEvent::KeyDown { key },
                    (true, false) => InputEvent::KeyUp { key },
                    (false, _) => input,
                }
            }
            None => input,
        };

        // A release after a page/profile switch belongs to the old action.
        let state = self.device_mut(device)?;
        match input {
            InputEvent::KeyDown { key } => {
                state.held_keys.insert(key);
            }
            InputEvent::KeyUp { key } => {
                state.held_keys.remove(&key);
                if state.swallow_release.remove(&key) {
                    return Ok(());
                }
            }
            _ => {}
        }

        let (controller, position, event) = match input {
            InputEvent::KeyDown { key } => (Controller::Keypad, key, "keyDown"),
            InputEvent::KeyUp { key } => (Controller::Keypad, key, "keyUp"),
            InputEvent::EncoderDown { encoder } => (Controller::Encoder, encoder, "dialDown"),
            InputEvent::EncoderUp { encoder } => (Controller::Encoder, encoder, "dialUp"),
            InputEvent::EncoderTwist { encoder, ticks } => {
                let pressed = self.device(device)?.encoders_pressed[encoder as usize];
                if self.is_builtin(device, Controller::Encoder, encoder)? {
                    builtin::on_input(self, device, input).await;
                    return Ok(());
                }
                self.send_slot_event(device, Controller::Encoder, encoder, |slot| {
                    slot.dial_rotate(ticks, pressed)
                })
                .await;
                return Ok(());
            }
        };

        if self.is_builtin(device, controller, position)? {
            builtin::on_input(self, device, input).await;
            return Ok(());
        }
        self.send_slot_event(device, controller, position, |slot| slot.simple(event))
            .await;
        Ok(())
    }

    fn set_encoder_pressed(
        &mut self,
        device: &str,
        encoder: u8,
        pressed: bool,
    ) -> anyhow::Result<()> {
        let state = self.device_mut(device)?;
        let slot = state
            .encoders_pressed
            .get_mut(encoder as usize)
            .context("encoder out of range")?;
        *slot = pressed;
        Ok(())
    }

    fn is_builtin(
        &self,
        device: &str,
        controller: Controller,
        position: u8,
    ) -> anyhow::Result<bool> {
        Ok(self
            .device(device)?
            .page()
            .slots(controller)
            .get(&position)
            .is_some_and(|i| i.plugin == BUILTIN_PLUGIN))
    }

    // ---------------------------------------------------------------------
    // Profiles & rendering
    // ---------------------------------------------------------------------

    /// Sends `willAppear` for every bound slot and draws all keys.
    async fn activate_profile(&mut self, device: &str) {
        let Some(state) = self.devices.get(device) else {
            return;
        };
        for (controller, position) in state.bound_slots() {
            self.send_slot_event(device, controller, position, |s| s.simple("willAppear"))
                .await;
        }
        let keys = state.handle.info.layout.display_keys;
        for key in 0..keys {
            self.redraw_key(device, key).await;
        }
    }

    async fn deactivate_profile(&mut self, device: &str) {
        let Some(state) = self.devices.get(device) else {
            return;
        };
        for (controller, position) in state.bound_slots() {
            self.send_slot_event(device, controller, position, |s| s.simple("willDisappear"))
                .await;
        }
        if let Some(state) = self.devices.get_mut(device) {
            state.titles.clear();
            state.runtime_images.clear();
            let held = std::mem::take(&mut state.held_keys);
            state.swallow_release.extend(held);
        }
    }

    pub async fn switch_profile(&mut self, device: &str, profile_id: &str) -> anyhow::Result<()> {
        let profile = self.store.load_profile(device, profile_id)?;
        self.store.save_profile(device, &profile)?;
        self.deactivate_profile(device).await;

        let state = self.device_mut(device)?;
        state.profile = profile;
        state.page = 0;
        state.config.active_profile = profile_id.to_owned();
        let config = state.config.clone();
        self.store.save_device_config(device, &config)?;

        self.activate_profile(device).await;
        self.emit(json!({ "event": "profileChanged", "device": self.device_snapshot(device)? }));
        Ok(())
    }

    /// Switches to the profile after the active one in the profile list, wrapping around.
    pub async fn next_profile(&mut self, device: &str) -> anyhow::Result<()> {
        let ids = self.store.list_profiles(device)?;
        let active = &self.device(device)?.config.active_profile;
        let next = ids
            .iter()
            .position(|id| id == active)
            .map_or(0, |i| (i + 1) % ids.len());
        match ids.get(next).cloned() {
            Some(id) => self.switch_profile(device, &id).await,
            None => Ok(()),
        }
    }

    /// Shows page `index` of the active profile.
    pub async fn switch_page(&mut self, device: &str, index: usize) -> anyhow::Result<()> {
        let state = self.device(device)?;
        let count = state.profile.pages.len();
        anyhow::ensure!(index < count, "Seite {} gibt es nicht", index + 1);
        if index == state.page {
            return Ok(());
        }
        self.deactivate_profile(device).await;
        self.device_mut(device)?.page = index;
        self.activate_profile(device).await;
        self.emit(json!({ "event": "pageChanged", "device": self.device_snapshot(device)? }));
        Ok(())
    }

    /// Moves `steps` pages forward (negative: back), wrapping around.
    pub async fn step_page(&mut self, device: &str, steps: i64) -> anyhow::Result<()> {
        let state = self.device(device)?;
        let count = state.profile.pages.len() as i64;
        let index = (state.page as i64 + steps).rem_euclid(count);
        self.switch_page(device, index as usize).await
    }

    fn save_active_profile(&mut self, device: &str) -> anyhow::Result<()> {
        let profile = self.device(device)?.profile.clone();
        self.store.save_profile(device, &profile)
    }

    /// Image a key shows when no plugin has overridden it at runtime:
    /// user image → built-in icon / manifest state image → manifest action icon → blank.
    async fn default_image(&self, device: &str, key: u8) -> Option<DynamicImage> {
        let instance = self.devices.get(device)?.page().keys.get(&key)?;
        if let Some(url) = &instance.image {
            match render::decode_data_url(url) {
                Ok(image) => return Some(image),
                Err(err) => tracing::warn!(%err, "invalid user image"),
            }
        }
        if instance.plugin == BUILTIN_PLUGIN {
            return builtin::icon(instance);
        }
        let plugin = self
            .plugins
            .plugins()
            .await
            .into_iter()
            .find(|p| p.uuid == instance.plugin)?;
        let action = plugin
            .manifest
            .actions
            .iter()
            .find(|a| a.uuid == instance.action)?;
        let state_image = action
            .states
            .get(instance.state as usize)
            .map(|s| s.image.as_str())
            .unwrap_or_default();
        render::load_icon(&plugin.path, state_image)
            .or_else(|| render::load_icon(&plugin.path, &action.icon))
    }

    /// Title drawn on a key: user title, else the title a plugin set at runtime.
    /// Without either the key shows only its image.
    fn key_title(&self, device: &str, key: u8) -> Option<String> {
        let state = self.devices.get(device)?;
        let instance = state.page().keys.get(&key)?;
        instance
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| state.titles.get(&key).cloned())
    }

    /// Recomposes a display key from its image and title and sends it out.
    async fn redraw_key(&mut self, device: &str, key: u8) {
        let runtime = self
            .devices
            .get(device)
            .and_then(|s| s.runtime_images.get(&key).cloned());
        let base = match runtime {
            Some(image) => Some(image),
            None => self.default_image(device, key).await,
        };
        let title = self.key_title(device, key);
        let image = render::compose_key(base, title.as_deref());
        self.set_key_image(device, key, image).await;
    }

    async fn set_key_image(&mut self, device: &str, key: u8, image: Option<DynamicImage>) {
        let Some(state) = self.devices.get_mut(device) else {
            return;
        };
        if !state.handle.info.layout.has_display(key) {
            return;
        }
        let preview = image.as_ref().and_then(|i| render::to_png_data_url(i).ok());
        match &preview {
            Some(url) => state.previews.insert(key, url.clone()),
            None => state.previews.remove(&key),
        };
        state
            .handle
            .send(DeviceCommand::SetKeyImage {
                key,
                image: image.map(Arc::new),
            })
            .await;
        self.emit(json!({ "event": "keyImage", "device": device, "key": key, "image": preview }));
    }

    pub async fn set_brightness(&mut self, device: &str, value: u8) -> anyhow::Result<()> {
        let value = value.min(100);
        let state = self.device_mut(device)?;
        state.config.brightness = value;
        state.handle.send(DeviceCommand::SetBrightness(value)).await;
        let config = state.config.clone();
        self.store.save_device_config(device, &config)?;
        self.emit(json!({ "event": "brightnessChanged", "device": device, "value": value }));
        Ok(())
    }

    /// Mutates an instance of the *active* profile and persists the profile.
    fn update_instance(
        &mut self,
        device: &str,
        controller: Controller,
        position: u8,
        update: impl FnOnce(&mut ActionInstance),
    ) -> anyhow::Result<()> {
        let state = self.device_mut(device)?;
        let instance = state
            .page_mut()
            .slots_mut(controller)
            .get_mut(&position)
            .context("slot is empty")?;
        update(instance);
        let profile = state.profile.clone();
        self.store.save_profile(device, &profile)
    }

    // ---------------------------------------------------------------------
    // Plugins
    // ---------------------------------------------------------------------

    pub async fn on_plugin_message(&mut self, msg: PluginMessage) {
        let plugin = msg.plugin;
        let result = match msg.event {
            PluginEvent::Registered => {
                self.plugin_registered(&plugin).await;
                Ok(())
            }
            PluginEvent::Disconnected => {
                self.plugin_requests
                    .retain(|_, (owner, _, _)| owner != &plugin);
                Ok(())
            }
            PluginEvent::Inbound(event) => self.on_plugin_event(&plugin, event).await,
        };
        if let Err(err) = result {
            tracing::warn!(%plugin, %err, "plugin event rejected");
        }
        let connected = self.plugins.is_connected(&plugin).await;
        self.emit(json!({ "event": "pluginStatus", "plugin": plugin, "connected": connected }));
    }

    async fn plugin_registered(&mut self, plugin: &str) {
        for state in self.devices.values() {
            self.plugins
                .send(plugin, &protocol::device_did_connect(&state.handle.info))
                .await;
            for (controller, position) in state.bound_slots() {
                if let Some((owner, event)) =
                    state.slot_event(controller, position, |s| s.simple("willAppear"))
                    && owner == plugin
                {
                    self.plugins.send(plugin, &event).await;
                }
            }
        }
    }

    /// Resolves a context to a slot of an active profile owned by `plugin`.
    fn resolve_context(&self, plugin: &str, context: &str) -> anyhow::Result<SlotContext> {
        let slot = SlotContext::decode(context).context("malformed context")?;
        let state = self.device(&slot.device)?;
        if state.profile.id != slot.profile || state.page != usize::from(slot.page) {
            bail!("context belongs to an inactive profile or page");
        }
        let instance = state
            .page()
            .slots(slot.controller)
            .get(&slot.position)
            .context("slot is empty")?;
        if instance.plugin != plugin {
            bail!("context belongs to another plugin");
        }
        Ok(slot)
    }

    async fn on_plugin_event(&mut self, plugin: &str, event: InboundEvent) -> anyhow::Result<()> {
        match event {
            InboundEvent::SetSettings { context, payload } => {
                let slot = self.resolve_context(plugin, &context)?;
                self.update_instance(&slot.device, slot.controller, slot.position, |i| {
                    i.settings = payload
                })?;
                self.emit(json!({ "event": "slotChanged", "device": slot.device }));
            }
            InboundEvent::GetSettings { context } => {
                let slot = self.resolve_context(plugin, &context)?;
                self.send_slot_event(&slot.device, slot.controller, slot.position, |s| {
                    s.simple("didReceiveSettings")
                })
                .await;
            }
            InboundEvent::SetGlobalSettings { context, payload } => {
                anyhow::ensure!(context == plugin, "global settings context mismatch");
                self.store.save_global_settings(plugin, &payload)?;
                self.emit(json!({ "event": "globalSettingsChanged", "plugin": plugin }));
            }
            InboundEvent::GetGlobalSettings { context } => {
                anyhow::ensure!(context == plugin, "global settings context mismatch");
                let settings = self.store.load_global_settings(plugin)?;
                self.plugins
                    .send(plugin, &protocol::did_receive_global_settings(&settings))
                    .await;
            }
            InboundEvent::SetTitle { context, payload } => {
                let slot = self.resolve_context(plugin, &context)?;
                if slot.controller == Controller::Keypad {
                    let state = self.device_mut(&slot.device)?;
                    match payload.title.clone().filter(|t| !t.is_empty()) {
                        Some(title) => state.titles.insert(slot.position, title),
                        None => state.titles.remove(&slot.position),
                    };
                    self.redraw_key(&slot.device, slot.position).await;
                }
                self.emit(json!({
                    "event": "keyTitle", "device": slot.device,
                    "controller": slot.controller, "position": slot.position,
                    "title": payload.title,
                }));
            }
            InboundEvent::SetImage { context, payload } => {
                let slot = self.resolve_context(plugin, &context)?;
                if slot.controller != Controller::Keypad {
                    return Ok(());
                }
                let image = match payload.image.as_deref().filter(|s| !s.is_empty()) {
                    Some(url) => Some(render::decode_data_url(url)?),
                    None => None,
                };
                let state = self.device_mut(&slot.device)?;
                match image {
                    Some(image) => state.runtime_images.insert(slot.position, image),
                    None => state.runtime_images.remove(&slot.position),
                };
                self.redraw_key(&slot.device, slot.position).await;
            }
            InboundEvent::SetState { context, payload } => {
                let slot = self.resolve_context(plugin, &context)?;
                self.update_instance(&slot.device, slot.controller, slot.position, |i| {
                    i.state = payload.state
                })?;
                if slot.controller == Controller::Keypad {
                    // A state change shows the state's image again.
                    self.device_mut(&slot.device)?
                        .runtime_images
                        .remove(&slot.position);
                    self.redraw_key(&slot.device, slot.position).await;
                }
            }
            InboundEvent::ShowOk { context } | InboundEvent::ShowAlert { context } => {
                let slot = self.resolve_context(plugin, &context)?;
                self.emit(json!({
                    "event": "feedback", "device": slot.device,
                    "controller": slot.controller, "position": slot.position,
                }));
            }
            InboundEvent::LogMessage { payload } => {
                tracing::info!(target: "plugin", %plugin, "{}", payload.message);
            }
            InboundEvent::OpenUrl { payload } => open_url(&payload.url)?,
            InboundEvent::SendToPropertyInspector { payload, .. } => {
                let id = payload["requestId"].as_u64().unwrap_or_default();
                match self.plugin_requests.remove(&id) {
                    Some((owner, _, reply)) if owner == plugin => {
                        let result = match payload["error"].as_str() {
                            Some(error) => Err(error.to_owned()),
                            None => Ok(payload),
                        };
                        reply.send(result).ok();
                    }
                    Some(entry) => {
                        self.plugin_requests.insert(id, entry);
                    }
                    None => {
                        tracing::debug!(%plugin, "sendToPropertyInspector without open request")
                    }
                }
            }
            InboundEvent::Unsupported => tracing::debug!(%plugin, "unsupported plugin event"),
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // UI API
    // ---------------------------------------------------------------------

    pub async fn on_api_command(&mut self, command: ApiCommand) -> anyhow::Result<Value> {
        match command {
            ApiCommand::GetState => {
                let mut ids: Vec<_> = self.devices.keys().cloned().collect();
                ids.sort();
                let devices = ids
                    .iter()
                    .map(|id| self.device_snapshot(id))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "devices": devices,
                    "catalog": self.catalog().await,
                }))
            }
            ApiCommand::GetCatalog => Ok(self.catalog().await),
            ApiCommand::SwitchProfile { device, profile } => {
                self.switch_profile(&device, &profile).await?;
                Ok(Value::Null)
            }
            ApiCommand::SwitchPage { device, page } => {
                self.switch_page(&device, page).await?;
                Ok(Value::Null)
            }
            ApiCommand::AddPage { device, name } => {
                let state = self.device_mut(&device)?;
                anyhow::ensure!(state.profile.pages.len() < 50, "höchstens 50 Seiten");
                state.profile.pages.push(Page {
                    name: name.unwrap_or_default().trim().to_owned(),
                    ..Default::default()
                });
                let index = state.profile.pages.len() - 1;
                self.save_active_profile(&device)?;
                self.switch_page(&device, index).await?;
                Ok(json!({ "page": index }))
            }
            ApiCommand::CopyPage { device, page } => {
                let pages = &self.device(&device)?.profile.pages;
                anyhow::ensure!(pages.len() < 50, "höchstens 50 Seiten");
                let mut copy = pages.get(page).context("Seite gibt es nicht")?.clone();
                copy.name = match copy.name.as_str() {
                    "" => format!("Seite {} (Kopie)", page + 1),
                    name => format!("{name} (Kopie)"),
                };
                // Contexts carry the page index: hide, insert after the original, show the copy.
                self.deactivate_profile(&device).await;
                let state = self.device_mut(&device)?;
                state.profile.pages.insert(page + 1, copy);
                state.page = page + 1;
                self.save_active_profile(&device)?;
                self.activate_profile(&device).await;
                self.emit(
                    json!({ "event": "pageChanged", "device": self.device_snapshot(&device)? }),
                );
                Ok(json!({ "page": page + 1 }))
            }
            ApiCommand::RenamePage { device, page, name } => {
                let state = self.device_mut(&device)?;
                state
                    .profile
                    .pages
                    .get_mut(page)
                    .context("Seite gibt es nicht")?
                    .name = name.trim().to_owned();
                self.save_active_profile(&device)?;
                self.emit(
                    json!({ "event": "pageChanged", "device": self.device_snapshot(&device)? }),
                );
                Ok(Value::Null)
            }
            ApiCommand::MovePage { device, page, to } => {
                let state = self.device(&device)?;
                let count = state.profile.pages.len();
                anyhow::ensure!(page < count && to < count, "Seite gibt es nicht");
                let shown = state.page;
                // Contexts carry the page index: hide, reorder, show again.
                self.deactivate_profile(&device).await;
                let state = self.device_mut(&device)?;
                let moved = state.profile.pages.remove(page);
                state.profile.pages.insert(to, moved);
                state.page = if shown == page {
                    to
                } else if page < shown && to >= shown {
                    shown - 1
                } else if page > shown && to <= shown {
                    shown + 1
                } else {
                    shown
                };
                self.save_active_profile(&device)?;
                self.activate_profile(&device).await;
                self.emit(
                    json!({ "event": "pageChanged", "device": self.device_snapshot(&device)? }),
                );
                Ok(Value::Null)
            }
            ApiCommand::DeletePage { device, page } => {
                let state = self.device(&device)?;
                let count = state.profile.pages.len();
                anyhow::ensure!(page < count, "Seite gibt es nicht");
                anyhow::ensure!(count > 1, "die letzte Seite kann nicht gelöscht werden");
                let shown = state.page;
                self.deactivate_profile(&device).await;
                let state = self.device_mut(&device)?;
                state.profile.pages.remove(page);
                if shown > page || shown == count - 1 {
                    state.page = shown.saturating_sub(1);
                }
                self.save_active_profile(&device)?;
                self.activate_profile(&device).await;
                self.emit(
                    json!({ "event": "pageChanged", "device": self.device_snapshot(&device)? }),
                );
                Ok(Value::Null)
            }
            ApiCommand::CopyProfile {
                device,
                profile,
                to,
            } => {
                let existing = self.store.list_profiles(&device)?;
                anyhow::ensure!(
                    existing.contains(&profile),
                    "Profil „{profile}“ gibt es nicht"
                );
                anyhow::ensure!(!existing.contains(&to), "Profil „{to}“ gibt es schon");
                let mut copy = self.store.load_profile(&device, &profile)?;
                copy.id = to.clone();
                copy.name = to;
                self.store.save_profile(&device, &copy)?;
                self.emit(
                    json!({ "event": "profileChanged", "device": self.device_snapshot(&device)? }),
                );
                Ok(Value::Null)
            }
            ApiCommand::DeleteProfile { device, profile } => {
                anyhow::ensure!(
                    self.device(&device)?.profile.id != profile,
                    "cannot delete the active profile"
                );
                self.store.delete_profile(&device, &profile)?;
                self.emit(
                    json!({ "event": "profileChanged", "device": self.device_snapshot(&device)? }),
                );
                Ok(Value::Null)
            }
            ApiCommand::SetAction {
                device,
                controller,
                position,
                plugin,
                action,
                settings,
            } => {
                if plugin == BUILTIN_PLUGIN {
                    anyhow::ensure!(builtin::exists(&action), "unknown built-in action {action}");
                }
                self.clear_slot(&device, controller, position).await?;
                let settings = match settings {
                    Some(settings) => Some(settings),
                    None if plugin == BUILTIN_PLUGIN => Some(builtin::default_settings(&action)),
                    None => self.plugins.get(&plugin).await.and_then(|p| {
                        let schema = p
                            .manifest
                            .actions
                            .iter()
                            .find(|a| a.uuid == action)?
                            .settings_schema
                            .clone()?;
                        Some(builtin::schema_defaults(&schema))
                    }),
                };
                let mut instance = ActionInstance::new(plugin, action);
                if let Some(settings) = settings {
                    instance.settings = settings;
                }
                self.put_slot(&device, controller, position, instance)
                    .await?;
                self.emit(json!({ "event": "slotChanged", "device": device }));
                Ok(Value::Null)
            }
            ApiCommand::SetSlot {
                device,
                controller,
                position,
                instance,
            } => {
                match instance {
                    Some(instance) => {
                        if instance.plugin == BUILTIN_PLUGIN {
                            anyhow::ensure!(
                                builtin::exists(&instance.action),
                                "unknown built-in action {}",
                                instance.action
                            );
                        }
                        self.clear_slot(&device, controller, position).await?;
                        self.put_slot(&device, controller, position, instance)
                            .await?;
                    }
                    None => {
                        self.clear_slot(&device, controller, position).await?;
                        if controller == Controller::Keypad {
                            self.set_key_image(&device, position, None).await;
                        }
                    }
                }
                self.emit(json!({ "event": "slotChanged", "device": device }));
                Ok(Value::Null)
            }
            ApiCommand::ClearAction {
                device,
                controller,
                position,
            } => {
                self.clear_slot(&device, controller, position).await?;
                if controller == Controller::Keypad {
                    self.set_key_image(&device, position, None).await;
                }
                self.emit(json!({ "event": "slotChanged", "device": device }));
                Ok(Value::Null)
            }
            ApiCommand::SetActionSettings {
                device,
                controller,
                position,
                settings,
            } => {
                self.update_instance(&device, controller, position, |i| i.settings = settings)?;
                self.send_slot_event(&device, controller, position, |s| {
                    s.simple("didReceiveSettings")
                })
                .await;
                if controller == Controller::Keypad
                    && self.is_builtin(&device, controller, position)?
                {
                    // Label and icon of built-ins depend on their settings.
                    self.redraw_key(&device, position).await;
                }
                self.emit(json!({ "event": "slotChanged", "device": device }));
                Ok(Value::Null)
            }
            ApiCommand::SetActionAppearance {
                device,
                controller,
                position,
                title,
                image,
            } => {
                self.update_instance(&device, controller, position, |i| {
                    i.title = title;
                    i.image = image;
                })?;
                if controller == Controller::Keypad {
                    self.redraw_key(&device, position).await;
                }
                self.emit(json!({ "event": "slotChanged", "device": device }));
                Ok(Value::Null)
            }
            ApiCommand::SetBrightness { device, value } => {
                self.set_brightness(&device, value).await?;
                Ok(Value::Null)
            }
            ApiCommand::SimulateInput { device, input } => {
                self.device(&device)?;
                self.on_device_event(DeviceEvent::Input {
                    device,
                    event: input,
                })
                .await;
                Ok(Value::Null)
            }
            ApiCommand::StartVirtualDevice => {
                if !self
                    .devices
                    .contains_key(n3_driver::virtual_deck::VIRTUAL_DEVICE_ID)
                {
                    tokio::spawn(n3_driver::virtual_deck::run_virtual_device(
                        self.device_events.clone(),
                        self.shutdown.child_token(),
                    ));
                }
                Ok(json!({ "device": n3_driver::virtual_deck::VIRTUAL_DEVICE_ID }))
            }
            ApiCommand::GetAppSettings => {
                Ok(serde_json::to_value(self.store.load_app_settings()?)?)
            }
            ApiCommand::SetAppSettings { settings } => {
                let mut current = serde_json::to_value(self.store.load_app_settings()?)?;
                if let (Some(target), Value::Object(patch)) = (current.as_object_mut(), settings) {
                    for (key, value) in patch {
                        target.insert(key, value);
                    }
                }
                let settings: crate::store::AppSettings =
                    serde_json::from_value(current).context("ungültige Einstellungen")?;
                self.store.save_app_settings(&settings)?;
                let value = serde_json::to_value(&settings)?;
                self.emit(json!({ "event": "appSettingsChanged", "settings": value }));
                Ok(value)
            }
            ApiCommand::GetGlobalSettings { plugin } => self.store.load_global_settings(&plugin),
            ApiCommand::SetGlobalSettings { plugin, settings } => {
                anyhow::ensure!(
                    self.plugins.get(&plugin).await.is_some(),
                    "plugin is not installed"
                );
                // Merge, so values the plugin stored itself (e.g. tokens) survive.
                let mut merged = self.store.load_global_settings(&plugin)?;
                if let (Some(target), Value::Object(patch)) = (merged.as_object_mut(), settings) {
                    for (key, value) in patch {
                        target.insert(key, value);
                    }
                }
                self.store.save_global_settings(&plugin, &merged)?;
                self.plugins
                    .send(&plugin, &protocol::did_receive_global_settings(&merged))
                    .await;
                self.emit(json!({ "event": "globalSettingsChanged", "plugin": plugin }));
                Ok(merged)
            }
            ApiCommand::ActivatePlugin { staged } => self.activate_plugin(&staged).await,
            ApiCommand::ReportPluginUpdates { plugins } => {
                self.emit(json!({ "event": "pluginsUpdated", "plugins": plugins }));
                Ok(Value::Null)
            }
            ApiCommand::UninstallPlugin { plugin } => self.uninstall_plugin(&plugin).await,
            ApiCommand::InstallPlugin { .. }
            | ApiCommand::PluginStore { .. }
            | ApiCommand::UpdatePlugins
            | ApiCommand::PluginRequest { .. } => {
                bail!("handled outside on_api_command")
            }
        }
    }

    /// Places `instance` on an empty slot, saves and shows it.
    async fn put_slot(
        &mut self,
        device: &str,
        controller: Controller,
        position: u8,
        instance: ActionInstance,
    ) -> anyhow::Result<()> {
        let state = self.device_mut(device)?;
        state
            .page_mut()
            .slots_mut(controller)
            .insert(position, instance);
        let profile = state.profile.clone();
        self.store.save_profile(device, &profile)?;
        self.send_slot_event(device, controller, position, |s| s.simple("willAppear"))
            .await;
        if controller == Controller::Keypad {
            self.redraw_key(device, position).await;
        }
        Ok(())
    }

    async fn clear_slot(
        &mut self,
        device: &str,
        controller: Controller,
        position: u8,
    ) -> anyhow::Result<()> {
        self.send_slot_event(device, controller, position, |s| s.simple("willDisappear"))
            .await;
        let state = self.device_mut(device)?;
        if state
            .page_mut()
            .slots_mut(controller)
            .remove(&position)
            .is_some()
        {
            if controller == Controller::Keypad {
                state.titles.remove(&position);
                state.runtime_images.remove(&position);
            }
            let profile = state.profile.clone();
            self.store.save_profile(device, &profile)?;
        }
        Ok(())
    }

    pub fn device_snapshot(&self, id: &str) -> anyhow::Result<Value> {
        let state = self.device(id)?;
        Ok(json!({
            "info": state.handle.info,
            "brightness": state.config.brightness,
            "activeProfile": state.profile.id,
            "profiles": self.store.list_profiles(id)?,
            // The page shown, in the shape of a single-page profile.
            "profile": {
                "id": state.profile.id,
                "name": state.profile.name,
                "keys": state.page().keys,
                "encoders": state.page().encoders,
            },
            "page": state.page,
            "pages": state.profile.pages.iter().map(|p| json!({
                "name": p.name,
                "keys": p.keys.len(),
                "encoders": p.encoders.len(),
            })).collect::<Vec<_>>(),
            "previews": state.previews,
            "titles": state.titles,
        }))
    }

    /// Moves an unpacked plugin into the plugins directory (replacing an
    /// older version) and starts it.
    async fn activate_plugin(&mut self, staged: &Path) -> anyhow::Result<Value> {
        let manifest = n3_plugin::PluginManifest::read(staged)?;
        anyhow::ensure!(
            manifest.uuid.is_some() || staged.extension().is_some_and(|e| e == "sdPlugin"),
            "manifest.json needs a UUID (or the plugin folder must be named <uuid>.sdPlugin)"
        );
        let uuid = n3_plugin::plugin_uuid(&manifest, staged);
        crate::plugin_install::validate_uuid(&uuid)?;
        let target = self.plugins_dir.join(format!("{uuid}.sdPlugin"));

        let previous = self.plugins.get(&uuid).await;
        if let Some(previous) = &previous {
            self.plugins.stop(&uuid).await;
            if previous.path.starts_with(&self.plugins_dir) && previous.path != target {
                remove_dir(&previous.path).await?;
            }
        }
        if target.exists() {
            remove_dir(&target).await?;
        }
        std::fs::rename(staged, &target)
            .with_context(|| format!("moving plugin to {}", target.display()))?;
        let plugin = self.plugins.add(&target).await?;
        let started = self.plugins.start_plugin(&uuid).await;
        if let Err(err) = &started {
            tracing::error!(plugin = %uuid, %err, "failed to start installed plugin");
        }
        tracing::info!(plugin = %uuid, version = %plugin.manifest.version, "plugin installed");
        self.emit(json!({ "event": "pluginsChanged", "plugin": uuid }));
        Ok(json!({
            "plugin": uuid,
            "name": plugin.manifest.name,
            "version": plugin.manifest.version,
            "previousVersion": previous.map(|p| p.manifest.version),
            "started": started.is_ok(),
            "startError": started.err().map(|e| format!("{e:#}")),
        }))
    }

    async fn uninstall_plugin(&mut self, uuid: &str) -> anyhow::Result<Value> {
        let plugin = self
            .plugins
            .get(uuid)
            .await
            .context("plugin is not installed")?;
        anyhow::ensure!(
            plugin.path.starts_with(&self.plugins_dir),
            "bundled plugins cannot be removed"
        );
        self.plugins.stop(uuid).await;
        self.plugins.remove(uuid).await;
        remove_dir(&plugin.path).await?;
        // A bundled copy with the same UUID takes over again.
        let mut fallback = None;
        for dir in &self.extra_plugin_dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for path in entries.flatten().map(|e| e.path()) {
                if let Ok(manifest) = n3_plugin::PluginManifest::read(&path)
                    && n3_plugin::plugin_uuid(&manifest, &path) == uuid
                {
                    fallback = Some(path);
                }
            }
        }
        if let Some(path) = fallback
            && self.plugins.add(&path).await.is_ok()
            && let Err(err) = self.plugins.start_plugin(uuid).await
        {
            tracing::error!(plugin = %uuid, %err, "failed to start bundled plugin");
        }
        tracing::info!(plugin = %uuid, "plugin removed");
        self.emit(json!({ "event": "pluginsChanged", "plugin": uuid }));
        Ok(json!({ "plugin": uuid }))
    }

    async fn catalog(&self) -> Value {
        let mut plugins = vec![builtin::catalog_entry()];
        for plugin in self.plugins.plugins().await {
            let actions: Vec<Value> = plugin
                .manifest
                .actions
                .iter()
                .map(|a| {
                    json!({
                        "uuid": a.uuid,
                        "name": a.name,
                        "tooltip": a.tooltip,
                        "controllers": a.controllers,
                        "settingsSchema": a.settings_schema,
                        "icon": render::load_icon(&plugin.path, &a.icon)
                            .and_then(|i| render::to_png_data_url(&i).ok()),
                    })
                })
                .collect();
            plugins.push(json!({
                "uuid": plugin.uuid,
                "name": plugin.manifest.name,
                "author": plugin.manifest.author,
                "version": plugin.manifest.version,
                "category": plugin.manifest.category,
                "description": plugin.manifest.description,
                "removable": plugin.path.starts_with(&self.plugins_dir),
                "globalSettingsSchema": plugin.manifest.global_settings_schema,
                "connected": self.plugins.is_connected(&plugin.uuid).await,
                "actions": actions,
            }));
        }
        Value::Array(plugins)
    }
}

/// Opens an http(s) URL with the platform's default handler.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        url.starts_with("https://") || url.starts_with("http://"),
        "refusing to open non-http url"
    );
    let program = match std::env::consts::OS {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    };
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .with_context(|| format!("running {program}"))?;
    Ok(())
}

/// `remove_dir_all` with retries: on Windows a just-killed plugin process can
/// hold its files for a moment.
async fn remove_dir(path: &Path) -> anyhow::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) if attempt >= 10 => {
                return Err(err).with_context(|| format!("removing {}", path.display()));
            }
            Err(_) => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

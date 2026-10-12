#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 MIDI plugin: sends notes, controller values and program changes
//! to a DAW (and MIDI2LR), and shows incoming note and controller feedback on
//! the keys (see `docs/PLUGIN_MIDI.md`).

use std::{
    collections::HashMap,
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use anyhow::Context;
#[cfg(unix)]
use midir::os::unix::{VirtualInput, VirtualOutput};
use midir::{
    MidiInput, MidiInputConnection, MidiInputPort, MidiOutput, MidiOutputConnection, MidiOutputPort,
};
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

const APP_NAME: &str = "OpenDeckN3";
/// Port value for the virtual port the plugin creates itself (Linux only).
const VIRTUAL: &str = "virtual";
const VIRTUAL_LABEL: &str = "Virtueller Port OpenDeckN3";
const RETRY_DELAY: Duration = Duration::from_secs(5);
const RELATIVE_MAX: i64 = 63;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Note,
    Cc,
}

/// A note or controller message from the input port.
/// `value` is the velocity (0 for Note Off) or the controller value.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Incoming {
    kind: Kind,
    channel: u8,
    number: u8,
    value: u8,
}

enum Msg {
    Status(String),
    Incoming(Incoming),
}

enum Job {
    Configure { output: String, input: String },
    Send { context: String, bytes: Vec<u8> },
}

/// Owns the MIDI connections. midir connections stay on this thread; the
/// plugin only sends jobs to it.
struct Ports {
    host: Host,
    msgs: UnboundedSender<Msg>,
    output_name: String,
    input_name: String,
    output: Option<MidiOutputConnection>,
    input: Option<MidiInputConnection<()>>,
}

impl Ports {
    fn configure(&mut self, output: String, input: String) {
        if output != self.output_name {
            self.output = None;
            self.output_name = output;
        }
        if input != self.input_name {
            self.input = None;
            self.input_name = input;
        }
        self.reconnect();
    }

    /// Opens every selected port that is not open yet and reports the status.
    fn reconnect(&mut self) {
        if self.output.is_none() && !self.output_name.is_empty() {
            self.output = open_output(&self.output_name).ok();
        }
        if self.input.is_none() && !self.input_name.is_empty() {
            self.input = open_input(&self.input_name, self.msgs.clone()).ok();
        }
        self.msgs.send(Msg::Status(self.status())).ok();
    }

    fn send(&mut self, context: &str, bytes: &[u8]) {
        let sent = self
            .output
            .as_mut()
            .is_some_and(|out| out.send(bytes).is_ok());
        if !sent {
            self.output = None;
            self.host.alert(context);
            self.host
                .log("MIDI-Nachricht nicht gesendet: MIDI-Ausgang nicht verfügbar");
            self.reconnect();
        }
    }

    fn status(&self) -> String {
        let output = if self.output_name.is_empty() {
            "Kein Ausgang gewählt".to_owned()
        } else {
            port_status("Ausgang", &self.output_name, self.output.is_some())
        };
        if self.input_name.is_empty() {
            return output;
        }
        format!(
            "{output} · {}",
            port_status("Eingang", &self.input_name, self.input.is_some())
        )
    }
}

fn port_status(label: &str, name: &str, open: bool) -> String {
    if open {
        format!("{label}: „{}“ verbunden", port_label(name))
    } else {
        format!(
            "{label}: „{}“ nicht verfügbar, neuer Versuch in {} s",
            port_label(name),
            RETRY_DELAY.as_secs()
        )
    }
}

fn port_label(name: &str) -> &str {
    if name == VIRTUAL { VIRTUAL_LABEL } else { name }
}

fn ports_thread(host: Host, msgs: UnboundedSender<Msg>, jobs: mpsc::Receiver<Job>) {
    let mut ports = Ports {
        host,
        msgs,
        output_name: String::new(),
        input_name: String::new(),
        output: None,
        input: None,
    };
    let mut next_retry = Instant::now() + RETRY_DELAY;
    loop {
        match jobs.recv_timeout(next_retry.saturating_duration_since(Instant::now())) {
            Ok(Job::Configure { output, input }) => ports.configure(output, input),
            Ok(Job::Send { context, bytes }) => ports.send(&context, &bytes),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next_retry {
            ports.reconnect();
            next_retry = Instant::now() + RETRY_DELAY;
        }
    }
}

fn open_output(name: &str) -> anyhow::Result<MidiOutputConnection> {
    let midi = MidiOutput::new(APP_NAME)?;
    #[cfg(unix)]
    if name == VIRTUAL {
        return midi
            .create_virtual(APP_NAME)
            .map_err(|err| anyhow::anyhow!("{err}"));
    }
    let port = find_output(&midi, name)?;
    midi.connect(&port, APP_NAME)
        .map_err(|err| anyhow::anyhow!("{err}"))
}

fn open_input(name: &str, msgs: UnboundedSender<Msg>) -> anyhow::Result<MidiInputConnection<()>> {
    let midi = MidiInput::new(APP_NAME)?;
    let on_bytes = move |_stamp: u64, bytes: &[u8], _: &mut ()| {
        if let Some(msg) = parse(bytes) {
            msgs.send(Msg::Incoming(msg)).ok();
        }
    };
    #[cfg(unix)]
    if name == VIRTUAL {
        return midi
            .create_virtual(APP_NAME, on_bytes, ())
            .map_err(|err| anyhow::anyhow!("{err}"));
    }
    let port = find_input(&midi, name)?;
    midi.connect(&port, APP_NAME, on_bytes, ())
        .map_err(|err| anyhow::anyhow!("{err}"))
}

fn find_output(midi: &MidiOutput, name: &str) -> anyhow::Result<MidiOutputPort> {
    midi.ports()
        .into_iter()
        .find(|port| midi.port_name(port).is_ok_and(|n| n == name))
        .context("Port nicht gefunden")
}

fn find_input(midi: &MidiInput, name: &str) -> anyhow::Result<MidiInputPort> {
    midi.ports()
        .into_iter()
        .find(|port| midi.port_name(port).is_ok_and(|n| n == name))
        .context("Port nicht gefunden")
}

/// Dropdown entries for the output port.
fn output_options() -> anyhow::Result<Vec<(String, String)>> {
    let midi = MidiOutput::new(APP_NAME)?;
    let ports = midi
        .ports()
        .into_iter()
        .filter_map(|port| midi.port_name(&port).ok())
        .map(|name| (name.clone(), name));
    #[cfg(unix)]
    let ports = ports.chain(std::iter::once(virtual_option()));
    Ok(ports.collect())
}

/// Dropdown entries for the optional input port; an empty value means none.
fn input_options() -> anyhow::Result<Vec<(String, String)>> {
    let midi = MidiInput::new(APP_NAME)?;
    let ports = std::iter::once((String::new(), "keine".to_owned())).chain(
        midi.ports()
            .into_iter()
            .filter_map(|port| midi.port_name(&port).ok())
            .map(|name| (name.clone(), name)),
    );
    #[cfg(unix)]
    let ports = ports.chain(std::iter::once(virtual_option()));
    Ok(ports.collect())
}

#[cfg(unix)]
fn virtual_option() -> (String, String) {
    (VIRTUAL.to_owned(), VIRTUAL_LABEL.to_owned())
}

struct Plugin {
    host: Host,
    jobs: mpsc::Sender<Job>,
    contexts: Contexts,
    global: Map<String, Value>,
    /// Per key: velocity or 0 for notes, controller value for CC keys.
    values: HashMap<String, u8>,
}

impl Plugin {
    fn new(host: Host, jobs: mpsc::Sender<Job>) -> Self {
        Self {
            host,
            jobs,
            contexts: Contexts::default(),
            global: Map::new(),
            values: HashMap::new(),
        }
    }

    fn on_host(&mut self, msg: Value) {
        let applied = self.contexts.apply(&msg);
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "keyDown" | "dialDown" => self.act(&context, press),
            "keyUp" | "dialUp" => self.act(&context, |inst, _| release(inst)),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                self.act(&context, |inst, value| rotate(inst, value, ticks));
            }
            "willDisappear" => {
                self.values.remove(&context);
            }
            "sendToPlugin" => self.on_request(msg["payload"].clone()),
            _ => {}
        }
        if let Some(context) = applied {
            self.refresh(&context);
        }
    }

    fn on_global(&mut self, settings: Value) {
        self.global = settings.as_object().cloned().unwrap_or_default();
        // Idempotent: the status write-back echoes the settings and only missing ports are opened.
        self.jobs
            .send(Job::Configure {
                output: global_str(&self.global, "output"),
                input: global_str(&self.global, "input"),
            })
            .ok();
    }

    fn on_request(&self, req: Value) {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || {
            let options = match req["request"].as_str() {
                Some("outputs") => output_options().context("MIDI-Ausgänge nicht abrufbar"),
                Some("inputs") => input_options().context("MIDI-Eingänge nicht abrufbar"),
                other => Err(anyhow::anyhow!("unbekannte Anfrage {other:?}")),
            };
            host.reply_options(&req, options);
        });
    }

    /// Applies a key or dial event: sends the resulting message and stores the new value.
    fn act(&mut self, context: &str, change: impl FnOnce(&Instance, u8) -> Option<(Vec<u8>, u8)>) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let value = self.values.get(context).copied().unwrap_or(0);
        if let Some((bytes, next)) = change(inst, value) {
            self.values.insert(context.to_owned(), next);
            self.jobs
                .send(Job::Send {
                    context: context.to_owned(),
                    bytes,
                })
                .ok();
        }
        self.refresh(context);
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Status(status) => self.set_status(status),
            Msg::Incoming(msg) => self.on_incoming(msg),
        }
    }

    fn set_status(&mut self, status: String) {
        if self.global.get("status").and_then(Value::as_str) != Some(status.as_str()) {
            self.global.insert("status".into(), json!(status));
            self.host.save_global(&self.global);
        }
    }

    fn on_incoming(&mut self, msg: Incoming) {
        let matching: Vec<String> = self
            .contexts
            .iter()
            .filter(|(_, inst)| addresses(inst, &msg))
            .map(|(context, _)| context.clone())
            .collect();
        for context in matching {
            self.values.insert(context.clone(), msg.value);
            self.refresh(&context);
        }
    }

    fn refresh(&mut self, context: &str) {
        let value = self.values.get(context).copied().unwrap_or(0);
        self.contexts
            .show(&self.host, context, u16::from(value > 0), "");
    }
}

/// Channel setting 1–16 as the status nibble value 0–15.
fn channel_byte(setting: f64) -> u8 {
    (setting.round().clamp(1.0, 16.0) as u8) - 1
}

/// A data byte, clamped to 0–127.
fn data_byte(value: f64) -> u8 {
    value.round().clamp(0.0, 127.0) as u8
}

fn note_on(channel: u8, note: u8, velocity: u8) -> Vec<u8> {
    vec![0x90 | channel, note, velocity]
}

fn note_off(channel: u8, note: u8) -> Vec<u8> {
    vec![0x80 | channel, note, 0]
}

fn control_change(channel: u8, controller: u8, value: u8) -> Vec<u8> {
    vec![0xB0 | channel, controller, value]
}

fn program_change(channel: u8, program: u8) -> Vec<u8> {
    vec![0xC0 | channel, program]
}

/// Relative encoding in two's complement around 0: +n is sent as n, -n as 128 - n.
fn relative_value(steps: i64) -> u8 {
    let steps = steps.clamp(-RELATIVE_MAX, RELATIVE_MAX);
    if steps < 0 {
        (128 + steps) as u8
    } else {
        steps as u8
    }
}

fn absolute_value(value: u8, steps: i64) -> u8 {
    (i64::from(value) + steps).clamp(0, 127) as u8
}

/// Parses note and controller messages; everything else is ignored.
fn parse(bytes: &[u8]) -> Option<Incoming> {
    let [status, number, value, ..] = *bytes else {
        return None;
    };
    let channel = status & 0x0F;
    let (kind, value) = match status & 0xF0 {
        // Note On with velocity 0 is a Note Off.
        0x90 => (Kind::Note, value),
        0x80 => (Kind::Note, 0),
        0xB0 => (Kind::Cc, value),
        _ => return None,
    };
    Some(Incoming {
        kind,
        channel,
        number,
        value,
    })
}

/// Whether an incoming message belongs to the note or controller of a key.
fn addresses(inst: &Instance, msg: &Incoming) -> bool {
    let (kind, number) = match inst.kind() {
        "note" => (Kind::Note, inst.number("note", 60.0)),
        "cc" => (Kind::Cc, inst.number("cc", 1.0)),
        _ => return false,
    };
    msg.kind == kind
        && msg.channel == channel_byte(inst.number("channel", 1.0))
        && msg.number == data_byte(number)
}

/// Messages for a key press and the value the key holds afterwards.
fn press(inst: &Instance, value: u8) -> Option<(Vec<u8>, u8)> {
    let channel = channel_byte(inst.number("channel", 1.0));
    match inst.kind() {
        "note" => {
            let note = data_byte(inst.number("note", 60.0));
            if inst.text("mode") == "toggle" && value > 0 {
                Some((note_off(channel, note), 0))
            } else {
                let velocity = data_byte(inst.number("velocity", 127.0));
                Some((note_on(channel, note, velocity), velocity))
            }
        }
        "cc" => {
            let controller = data_byte(inst.number("cc", 1.0));
            let next = if value > 0 { 0 } else { 127 };
            Some((control_change(channel, controller, next), next))
        }
        "program" => {
            let program = data_byte(inst.number("program", 0.0));
            Some((program_change(channel, program), value))
        }
        _ => None,
    }
}

/// Momentary notes end on key release.
fn release(inst: &Instance) -> Option<(Vec<u8>, u8)> {
    if inst.kind() != "note" || inst.text("mode") == "toggle" {
        return None;
    }
    let channel = channel_byte(inst.number("channel", 1.0));
    let note = data_byte(inst.number("note", 60.0));
    Some((note_off(channel, note), 0))
}

/// Dial steps on a CC key: absolute mode keeps the value, relative mode sends the step.
fn rotate(inst: &Instance, value: u8, ticks: i64) -> Option<(Vec<u8>, u8)> {
    if inst.kind() != "cc" || ticks == 0 {
        return None;
    }
    let channel = channel_byte(inst.number("channel", 1.0));
    let controller = data_byte(inst.number("cc", 1.0));
    let steps = ticks.saturating_mul(inst.number("step", 1.0).round() as i64);
    if inst.text("mode") == "relative" {
        Some((
            control_change(channel, controller, relative_value(steps)),
            value,
        ))
    } else {
        let next = absolute_value(value, steps);
        (next != value).then(|| (control_change(channel, controller, next), next))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (msg_tx, mut msg_rx) = unbounded_channel::<Msg>();
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let ports_host = host.clone();
    std::thread::spawn(move || ports_thread(ports_host, msg_tx, job_rx));
    let mut plugin = Plugin::new(host, job_tx);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg),
                None => break,
            },
            Some(msg) = msg_rx.recv() => plugin.on_msg(msg),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(action: &str, settings: Value) -> Contexts {
        let mut contexts = Contexts::default();
        contexts.apply(
            &json!({ "event": "willAppear", "context": "k", "action": action,
            "payload": { "controller": "Keypad", "settings": settings } }),
        );
        contexts
    }

    fn with_key<T>(action: &str, settings: Value, check: impl FnOnce(&Instance) -> T) -> T {
        let contexts = instance(action, settings);
        check(contexts.get("k").unwrap())
    }

    #[test]
    fn channel_and_data_are_clamped() {
        assert_eq!(channel_byte(1.0), 0);
        assert_eq!(channel_byte(16.0), 15);
        assert_eq!(channel_byte(0.0), 0);
        assert_eq!(channel_byte(99.0), 15);
        assert_eq!(data_byte(-5.0), 0);
        assert_eq!(data_byte(200.0), 127);
        assert_eq!(data_byte(60.4), 60);
    }

    #[test]
    fn messages_carry_channel_in_status_nibble() {
        assert_eq!(note_on(2, 60, 127), vec![0x92, 60, 127]);
        assert_eq!(note_off(15, 60), vec![0x8F, 60, 0]);
        assert_eq!(control_change(0, 7, 64), vec![0xB0, 7, 64]);
        assert_eq!(program_change(9, 5), vec![0xC9, 5]);
    }

    #[test]
    fn relative_steps_are_two_complement_and_clamped() {
        assert_eq!(relative_value(3), 3);
        assert_eq!(relative_value(-2), 126);
        assert_eq!(relative_value(-1), 127);
        assert_eq!(relative_value(100), 63);
        assert_eq!(relative_value(-100), 65);
    }

    #[test]
    fn absolute_values_stay_in_range() {
        assert_eq!(absolute_value(120, 20), 127);
        assert_eq!(absolute_value(3, -10), 0);
        assert_eq!(absolute_value(64, 2), 66);
    }

    #[test]
    fn incoming_bytes_are_parsed() {
        assert_eq!(
            parse(&[0x91, 60, 100]),
            Some(Incoming {
                kind: Kind::Note,
                channel: 1,
                number: 60,
                value: 100
            })
        );
        assert_eq!(parse(&[0x91, 60, 0]).unwrap().value, 0);
        assert_eq!(parse(&[0x81, 60, 40]).unwrap().value, 0);
        assert_eq!(
            parse(&[0xB3, 7, 0]),
            Some(Incoming {
                kind: Kind::Cc,
                channel: 3,
                number: 7,
                value: 0
            })
        );
        assert_eq!(parse(&[0xF8]), None);
        assert_eq!(parse(&[0xC0, 5]), None);
    }

    #[test]
    fn incoming_message_addresses_matching_key_only() {
        let note_key = instance("p.note", json!({ "channel": 2, "note": 36 }));
        let note_key = note_key.get("k").unwrap();
        let on = Incoming {
            kind: Kind::Note,
            channel: 1,
            number: 36,
            value: 90,
        };
        assert!(addresses(note_key, &on));
        assert!(!addresses(note_key, &Incoming { channel: 2, ..on }));
        assert!(!addresses(note_key, &Incoming { number: 37, ..on }));
        assert!(!addresses(
            note_key,
            &Incoming {
                kind: Kind::Cc,
                ..on
            }
        ));

        let cc_key = instance("p.cc", json!({ "channel": 1, "cc": 36 }));
        let cc_key = cc_key.get("k").unwrap();
        assert!(addresses(
            cc_key,
            &Incoming {
                kind: Kind::Cc,
                channel: 0,
                number: 36,
                value: 5
            }
        ));
        assert!(!addresses(cc_key, &on));
    }

    #[test]
    fn momentary_note_sends_on_press_and_off_on_release() {
        let settings = json!({ "channel": 1, "note": 60, "velocity": 100, "mode": "momentary" });
        with_key("p.note", settings, |inst| {
            assert_eq!(press(inst, 0), Some((vec![0x90, 60, 100], 100)));
            assert_eq!(release(inst), Some((vec![0x80, 60, 0], 0)));
        });
    }

    #[test]
    fn toggle_note_alternates_on_press() {
        let settings = json!({ "channel": 1, "note": 60, "mode": "toggle" });
        with_key("p.note", settings, |inst| {
            let (on, value) = press(inst, 0).unwrap();
            assert_eq!((on, value), (vec![0x90, 60, 127], 127));
            assert_eq!(press(inst, value), Some((vec![0x80, 60, 0], 0)));
            assert_eq!(release(inst), None);
        });
    }

    #[test]
    fn cc_press_toggles_between_zero_and_full() {
        with_key("p.cc", json!({ "channel": 1, "cc": 20 }), |inst| {
            assert_eq!(press(inst, 0), Some((vec![0xB0, 20, 127], 127)));
            assert_eq!(press(inst, 127), Some((vec![0xB0, 20, 0], 0)));
        });
    }

    #[test]
    fn absolute_dial_keeps_value_and_skips_limits() {
        with_key(
            "p.cc",
            json!({ "channel": 1, "cc": 20, "mode": "absolute", "step": 2 }),
            |inst| {
                assert_eq!(rotate(inst, 10, 3), Some((vec![0xB0, 20, 16], 16)));
                assert_eq!(rotate(inst, 127, 1), None);
            },
        );
    }

    #[test]
    fn relative_dial_sends_encoded_steps() {
        with_key(
            "p.cc",
            json!({ "channel": 1, "cc": 20, "mode": "relative", "step": 1 }),
            |inst| {
                assert_eq!(rotate(inst, 0, 3), Some((vec![0xB0, 20, 3], 0)));
                assert_eq!(rotate(inst, 0, -2), Some((vec![0xB0, 20, 126], 0)));
            },
        );
    }

    #[test]
    fn program_key_sends_program_change() {
        with_key(
            "p.program",
            json!({ "channel": 16, "program": 12 }),
            |inst| {
                assert_eq!(press(inst, 0), Some((vec![0xCF, 12], 0)));
                assert_eq!(rotate(inst, 0, 1), None);
            },
        );
    }
}

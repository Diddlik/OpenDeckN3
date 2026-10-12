#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 timer plugin: countdown, Pomodoro and stopwatch on keys and
//! dials (see `docs/PLUGIN_TIMER.md`). Runtime state is stored in the instance
//! settings as wall-clock times, so timers survive page and profile switches.

use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use n3_plugin_sdk::{Contexts, Host, Instance};
use serde_json::{Value, json};

const TICK: Duration = Duration::from_secs(1);
const MINUTE_MS: u64 = 60_000;
const MAX_MINUTES: f64 = 1440.0;
const LONG_PRESS_MS: u64 = 600;
const ALERT_MS: u64 = 10_000;

/// Time state of one timer. Times are Unix milliseconds, so a running timer
/// keeps counting while its key is not shown.
#[derive(Clone, Debug, Default, PartialEq)]
struct Clock {
    /// When the current run started; `None` while stopped.
    running_since: Option<u64>,
    /// Time counted before the current run (all of it while stopped).
    elapsed: u64,
    /// Pomodoro phase index; 0 is the first focus phase.
    phase: u64,
}

impl Clock {
    fn running(&self) -> bool {
        self.running_since.is_some()
    }

    fn elapsed_at(&self, now: u64) -> u64 {
        self.elapsed.saturating_add(
            self.running_since
                .map_or(0, |since| now.saturating_sub(since)),
        )
    }

    fn toggle(&mut self, now: u64) {
        match self.running_since.take() {
            Some(since) => self.elapsed = self.elapsed.saturating_add(now.saturating_sub(since)),
            None => self.running_since = Some(now),
        }
    }

    fn reached(&self, goal: u64, now: u64) -> bool {
        self.running() && self.elapsed_at(now) >= goal
    }
}

/// A placed timer: its clock, the end of the alert blink and a press in progress.
#[derive(Default)]
struct Timer {
    clock: Clock,
    alert_until: u64,
    /// `keyDown` time of a press in progress.
    pressed_at: Option<u64>,
}

impl Timer {
    fn from_settings(settings: &Value) -> Self {
        Self {
            clock: Clock {
                running_since: settings["_since"].as_u64(),
                elapsed: settings["_elapsed"].as_u64().unwrap_or(0),
                phase: settings["_phase"].as_u64().unwrap_or(0),
            },
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pomodoro {
    Focus,
    Short,
    Long,
}

/// Focus and break alternate; the last break of a cycle is the long one.
fn pomodoro_phase(index: u64, rounds: u64) -> Pomodoro {
    let period = 2 * rounds;
    let index = index % period;
    if index.is_multiple_of(2) {
        Pomodoro::Focus
    } else if index == period - 1 {
        Pomodoro::Long
    } else {
        Pomodoro::Short
    }
}

fn rounds(inst: &Instance) -> u64 {
    (inst.number("rounds", 4.0).clamp(1.0, 12.0) as u64).max(1)
}

/// Length of a setting in minutes, as ms. A malformed number (NaN) becomes one minute.
fn span(inst: &Instance, key: &str, default: f64) -> u64 {
    let minutes = inst.number(key, default).clamp(1.0, MAX_MINUTES);
    ((minutes * MINUTE_MS as f64) as u64).max(MINUTE_MS)
}

/// Length of the current run in ms; `None` for a stopwatch, which counts up.
fn goal(inst: &Instance, phase: u64) -> Option<u64> {
    match inst.kind() {
        "countdown" => Some(span(inst, "minutes", 5.0)),
        "pomodoro" => Some(match pomodoro_phase(phase, rounds(inst)) {
            Pomodoro::Focus => span(inst, "work", 25.0),
            Pomodoro::Short => span(inst, "short", 5.0),
            Pomodoro::Long => span(inst, "long", 15.0),
        }),
        _ => None,
    }
}

/// `mm:ss`, or `h:mm:ss` from one hour on.
fn clock_text(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Key state and title for the clock at `now`. Countdowns show whole seconds
/// rounded up, so the last second reads `00:01` and not `00:00` early.
fn appearance(inst: &Instance, clock: &Clock, now: u64) -> (u16, String) {
    let elapsed = clock.elapsed_at(now);
    match inst.kind() {
        "countdown" => {
            let left = goal(inst, clock.phase)
                .unwrap_or_default()
                .saturating_sub(elapsed);
            (u16::from(left == 0), clock_text(left.div_ceil(1000)))
        }
        "pomodoro" => {
            let phase = pomodoro_phase(clock.phase, rounds(inst));
            let left = goal(inst, clock.phase)
                .unwrap_or_default()
                .saturating_sub(elapsed);
            let label = if phase == Pomodoro::Focus {
                "Fokus"
            } else {
                "Pause"
            };
            (
                u16::from(phase != Pomodoro::Focus),
                format!("{label} {}", clock_text(left.div_ceil(1000))),
            )
        }
        _ => (0, clock_text(elapsed / 1000)),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct Plugin {
    host: Host,
    contexts: Contexts,
    timers: HashMap<String, Timer>,
}

impl Plugin {
    fn on_host(&mut self, msg: Value, now: u64) {
        let appeared = msg["event"] == "willAppear";
        if let Some(context) = self.contexts.apply(&msg) {
            // Only a new key loads its state; the echo of our own saves must not reset a running timer.
            if appeared {
                self.timers.insert(
                    context.clone(),
                    Timer::from_settings(&msg["payload"]["settings"]),
                );
                self.settle(&context, now);
            }
            self.refresh(&context, now);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "willDisappear" => {
                self.timers.remove(&context);
            }
            "keyDown" | "dialDown" => {
                if let Some(timer) = self.timers.get_mut(&context) {
                    timer.pressed_at = Some(now);
                }
            }
            "keyUp" | "dialUp" => self.release(&context, now),
            "dialRotate" => {
                let ticks = msg["payload"]["ticks"].as_i64().unwrap_or(0);
                self.rotate(&context, ticks, now);
            }
            _ => {}
        }
    }

    fn release(&mut self, context: &str, now: u64) {
        let Some(pressed) = self
            .timers
            .get_mut(context)
            .and_then(|t| t.pressed_at.take())
        else {
            return;
        };
        if now.saturating_sub(pressed) >= LONG_PRESS_MS {
            self.reset(context);
        } else {
            self.toggle(context, now);
        }
        self.refresh(context, now);
    }

    fn toggle(&mut self, context: &str, now: u64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let Some(timer) = self.timers.get_mut(context) else {
            return;
        };
        // A finished countdown starts over from its full length.
        let finished = goal(inst, timer.clock.phase)
            .is_some_and(|goal| !timer.clock.running() && timer.clock.elapsed_at(now) >= goal);
        if finished {
            timer.clock = Clock::default();
            timer.alert_until = 0;
        }
        timer.clock.toggle(now);
        self.persist(context, None);
    }

    fn reset(&mut self, context: &str) {
        let Some(timer) = self.timers.get_mut(context) else {
            return;
        };
        timer.clock = Clock::default();
        timer.alert_until = 0;
        self.persist(context, None);
    }

    /// Dials change the countdown length, but only while it is stopped.
    fn rotate(&mut self, context: &str, ticks: i64, now: u64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let Some(timer) = self.timers.get_mut(context) else {
            return;
        };
        if inst.kind() != "countdown" || timer.clock.running() || ticks == 0 {
            return;
        }
        let minutes = (inst.number("minutes", 5.0) + ticks as f64).clamp(1.0, MAX_MINUTES);
        timer.clock = Clock::default();
        timer.alert_until = 0;
        self.persist(context, Some(minutes));
        self.refresh(context, now);
    }

    /// Ends a run that reached its goal: a countdown stays at zero, a Pomodoro
    /// phase hands over to the next one. Both stop and alert.
    fn settle(&mut self, context: &str, now: u64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let Some(timer) = self.timers.get_mut(context) else {
            return;
        };
        let Some(goal) = goal(inst, timer.clock.phase) else {
            return;
        };
        if !timer.clock.reached(goal, now) {
            return;
        }
        timer.clock = if inst.kind() == "pomodoro" {
            let period = 2 * rounds(inst);
            Clock {
                phase: timer.clock.phase.wrapping_add(1) % period,
                ..Clock::default()
            }
        } else {
            Clock {
                elapsed: goal,
                ..Clock::default()
            }
        };
        timer.alert_until = now + ALERT_MS;
        self.host.alert(context);
        self.persist(context, None);
    }

    /// Saves the runtime state beside the user's settings. The host does not
    /// echo `setSettings`, so the local copy is updated here as well.
    fn persist(&mut self, context: &str, minutes: Option<f64>) {
        let Some(timer) = self.timers.get(context) else {
            return;
        };
        let mut settings = self
            .contexts
            .get(context)
            .and_then(|inst| inst.settings.as_object().cloned())
            .unwrap_or_default();
        if let Some(minutes) = minutes {
            settings.insert("minutes".into(), json!(minutes));
        }
        settings.insert("_since".into(), json!(timer.clock.running_since));
        settings.insert("_elapsed".into(), json!(timer.clock.elapsed));
        settings.insert("_phase".into(), json!(timer.clock.phase));
        self.contexts.apply(&json!({
            "event": "didReceiveSettings",
            "context": context,
            "payload": { "settings": settings },
        }));
        self.host.send(json!({
            "event": "setSettings",
            "context": context,
            "payload": settings,
        }));
    }

    fn refresh(&mut self, context: &str, now: u64) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let Some(timer) = self.timers.get(context) else {
            return;
        };
        let (state, title) = appearance(inst, &timer.clock, now);
        let blink_off = now < timer.alert_until && !(now / 1000).is_multiple_of(2);
        let title = if blink_off { String::new() } else { title };
        self.contexts.show(&self.host, context, state, &title);
    }

    fn on_tick(&mut self, now: u64) {
        let contexts: Vec<String> = self.timers.keys().cloned().collect();
        for context in contexts {
            self.settle(&context, now);
            self.refresh(&context, now);
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let mut plugin = Plugin {
        host,
        contexts: Contexts::default(),
        timers: HashMap::new(),
    };
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg, now_ms()),
                None => break,
            },
            _ = tick.tick() => plugin.on_tick(now_ms()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    const KEY: &str = "k";

    fn plugin() -> (Plugin, mpsc::UnboundedReceiver<Value>) {
        let (host, rx) = Host::channel("de.opendeckn3.timer");
        let plugin = Plugin {
            host,
            contexts: Contexts::default(),
            timers: HashMap::new(),
        };
        (plugin, rx)
    }

    fn appear(plugin: &mut Plugin, action: &str, settings: Value, now: u64) {
        plugin.on_host(
            json!({ "event": "willAppear", "context": KEY, "action": format!("de.opendeckn3.timer.{action}"),
                    "payload": { "controller": "Keypad", "settings": settings } }),
            now,
        );
    }

    fn instance(action: &str, settings: Value) -> Instance {
        let (mut plugin, _) = plugin();
        appear(&mut plugin, action, settings, 0);
        plugin.contexts.get(KEY).unwrap().clone()
    }

    fn events(rx: &mut mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn countdown_runs_out_and_pause_keeps_remaining_time() {
        let mut clock = Clock::default();
        clock.toggle(1_000);
        assert_eq!(clock.elapsed_at(31_000), 30_000);
        clock.toggle(31_000);
        assert_eq!(clock.elapsed_at(900_000), 30_000);
        assert!(!clock.reached(60_000, 900_000));
        clock.toggle(900_000);
        assert_eq!(clock.elapsed_at(930_000), 60_000);
        assert!(clock.reached(60_000, 930_000));
    }

    #[test]
    fn countdown_title_and_state() {
        let inst = instance("countdown", json!({ "minutes": 5 }));
        let mut clock = Clock::default();
        assert_eq!(appearance(&inst, &clock, 0), (0, "05:00".to_owned()));
        clock.toggle(0);
        assert_eq!(appearance(&inst, &clock, 1_000), (0, "04:59".to_owned()));
        assert_eq!(appearance(&inst, &clock, 300_000), (1, "00:00".to_owned()));
    }

    #[test]
    fn pomodoro_order_is_focus_break_and_long_break_after_rounds() {
        let order: Vec<Pomodoro> = (0..8).map(|i| pomodoro_phase(i, 4)).collect();
        use Pomodoro::{Focus as F, Long as L, Short as S};
        assert_eq!(order, [F, S, F, S, F, S, F, L]);
        assert_eq!(pomodoro_phase(8, 4), F);
        assert_eq!(pomodoro_phase(1, 1), L);
    }

    #[test]
    fn pomodoro_title_shows_phase_and_time() {
        let inst = instance(
            "pomodoro",
            json!({ "work": 25, "short": 5, "long": 15, "rounds": 4 }),
        );
        let mut clock = Clock::default();
        clock.toggle(0);
        assert_eq!(
            appearance(&inst, &clock, 10_000),
            (0, "Fokus 24:50".to_owned())
        );
        clock.phase = 1;
        assert_eq!(appearance(&inst, &clock, 0), (1, "Pause 05:00".to_owned()));
    }

    #[test]
    fn clock_text_uses_hours_from_one_hour_on() {
        assert_eq!(clock_text(0), "00:00");
        assert_eq!(clock_text(299), "04:59");
        assert_eq!(clock_text(3599), "59:59");
        assert_eq!(clock_text(3723), "1:02:03");
    }

    #[test]
    fn saved_echo_does_not_reset_running_timer() {
        let (mut plugin, mut rx) = plugin();
        appear(&mut plugin, "countdown", json!({ "minutes": 5 }), 0);
        plugin.on_host(json!({ "event": "keyDown", "context": KEY }), 1_000);
        plugin.on_host(json!({ "event": "keyUp", "context": KEY }), 1_100);
        let save = events(&mut rx)
            .into_iter()
            .find(|e| e["event"] == "setSettings")
            .unwrap();
        assert_eq!(save["payload"]["minutes"], 5);
        plugin.on_host(
            json!({ "event": "didReceiveSettings", "context": KEY, "payload": { "settings": save["payload"] } }),
            5_000,
        );
        let clock = &plugin.timers[KEY].clock;
        assert_eq!(clock.running_since, Some(1_100));
        assert_eq!(clock.elapsed_at(5_000), 3_900);
    }

    #[test]
    fn long_press_resets_and_short_press_toggles() {
        let (mut plugin, _rx) = plugin();
        appear(&mut plugin, "stopwatch", json!({}), 0);
        plugin.on_host(json!({ "event": "keyDown", "context": KEY }), 0);
        plugin.on_host(json!({ "event": "keyUp", "context": KEY }), 100);
        assert!(plugin.timers[KEY].clock.running());
        plugin.on_host(json!({ "event": "keyDown", "context": KEY }), 5_000);
        plugin.on_host(json!({ "event": "keyUp", "context": KEY }), 5_600);
        assert_eq!(plugin.timers[KEY].clock, Clock::default());
    }

    #[test]
    fn countdown_end_stops_alerts_and_stays_at_zero() {
        let (mut plugin, mut rx) = plugin();
        appear(&mut plugin, "countdown", json!({ "minutes": 1 }), 0);
        plugin.on_host(json!({ "event": "keyDown", "context": KEY }), 0);
        plugin.on_host(json!({ "event": "keyUp", "context": KEY }), 0);
        events(&mut rx);
        plugin.on_tick(60_000);
        let sent = events(&mut rx);
        assert!(sent.iter().any(|e| e["event"] == "showAlert"));
        assert!(
            sent.iter()
                .any(|e| e["event"] == "setState" && e["payload"]["state"] == 1)
        );
        assert!(!plugin.timers[KEY].clock.running());
        assert_eq!(plugin.timers[KEY].clock.elapsed, MINUTE_MS);
    }

    #[test]
    fn pomodoro_end_moves_to_next_phase() {
        let (mut plugin, _rx) = plugin();
        appear(&mut plugin, "pomodoro", json!({ "work": 25 }), 0);
        plugin.timers.get_mut(KEY).unwrap().clock.toggle(0);
        plugin.on_tick(25 * MINUTE_MS);
        let clock = &plugin.timers[KEY].clock;
        assert_eq!((clock.phase, clock.running()), (1, false));
    }

    #[test]
    fn dial_changes_minutes_only_while_stopped() {
        let (mut plugin, mut rx) = plugin();
        appear(&mut plugin, "countdown", json!({ "minutes": 5 }), 0);
        plugin.on_host(
            json!({ "event": "dialRotate", "context": KEY, "payload": { "ticks": 2 } }),
            0,
        );
        let save = events(&mut rx)
            .into_iter()
            .find(|e| e["event"] == "setSettings")
            .unwrap();
        assert_eq!(save["payload"]["minutes"], 7.0);
        // The host sends no echo; the next turn must build on the saved value.
        plugin.on_host(
            json!({ "event": "dialRotate", "context": KEY, "payload": { "ticks": 1 } }),
            0,
        );
        let save = events(&mut rx)
            .into_iter()
            .find(|e| e["event"] == "setSettings")
            .unwrap();
        assert_eq!(save["payload"]["minutes"], 8.0);

        plugin.timers.get_mut(KEY).unwrap().clock.toggle(0);
        plugin.on_host(
            json!({ "event": "dialRotate", "context": KEY, "payload": { "ticks": 1 } }),
            1_000,
        );
        assert!(events(&mut rx).is_empty());
    }
}

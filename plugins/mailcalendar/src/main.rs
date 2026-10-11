#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 mail & calendar plugin: unread mails over IMAP and the next
//! appointment from iCal addresses, without the Google APIs
//! (see `docs/PLUGIN_MAILKALENDER.md`).

mod ics;
mod imap;

use std::{collections::HashMap, time::Duration};

use chrono::Utc;
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_secs(3);
/// Mail is checked every 20 ticks (1 min), calendars every 100 (5 min).
const MAIL_EVERY: u64 = 20;
const CALENDAR_EVERY: u64 = 100;
/// How far ahead the next appointment is looked for.
const HORIZON_DAYS: i64 = 8;
const MAIL_URL: &str = "https://mail.google.com/";
const CALENDAR_URL: &str = "https://calendar.google.com/";

enum Msg {
    Host(Value),
    Mail(u64, anyhow::Result<HashMap<String, u32>>),
    Calendars(u64, Vec<(String, anyhow::Result<Vec<ics::Event>>)>),
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    contexts: Contexts,
    global: Map<String, Value>,
    /// Bumped on settings changes; results of older requests are dropped.
    generation: u64,
    account: Result<imap::Account, String>,
    unread: HashMap<String, u32>,
    mail_error: Option<String>,
    mail_busy: bool,
    /// Upcoming events per calendar name.
    events: HashMap<String, Vec<ics::Event>>,
    calendar_error: Option<String>,
    calendar_busy: bool,
    ticks: u64,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            host,
            tx,
            contexts: Contexts::default(),
            global: Map::new(),
            generation: 0,
            account: Err(String::new()),
            unread: HashMap::new(),
            mail_error: None,
            mail_busy: false,
            events: HashMap::new(),
            calendar_error: None,
            calendar_busy: false,
            ticks: 0,
        }
    }

    fn calendars(&self) -> Vec<(String, String)> {
        parse_calendars(&global_str(&self.global, "calendars"))
    }

    fn mail_configured(&self) -> bool {
        !global_str(&self.global, "imapServer").is_empty()
            || !global_str(&self.global, "imapUser").is_empty()
    }

    fn update_status(&mut self) {
        let mut parts = Vec::new();
        let calendars = self.calendars().len();
        if calendars > 0 {
            parts.push(match &self.calendar_error {
                Some(err) => format!("Kalender: {err}"),
                None if self.events.is_empty() => "Kalender: lädt …".into(),
                None => format!("Kalender: {calendars} geladen"),
            });
        }
        if self.mail_configured() {
            parts.push(match (&self.account, &self.mail_error) {
                (Err(err), _) => format!("Mail: {err}"),
                (_, Some(err)) => format!("Mail: {err}"),
                (Ok(a), None) if self.unread.is_empty() => {
                    format!("Mail: verbinde mit {} …", a.host)
                }
                (Ok(a), None) => format!("Mail: verbunden mit {}", a.host),
            });
        }
        let status = if parts.is_empty() {
            "Kalender-Adressen und/oder Mail-Zugang eintragen".to_owned()
        } else {
            parts.join(" · ")
        };
        if self.global.get("status").and_then(Value::as_str) != Some(status.as_str()) {
            self.global.insert("status".into(), json!(status));
            self.host.save_global(&self.global);
        }
    }

    fn on_global(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        let changed = |k: &str| previous.get(k) != self.global.get(k);
        let mail = ["imapServer", "imapUser", "imapPassword", "connect"]
            .into_iter()
            .any(changed);
        let calendar = ["calendars", "connect"].into_iter().any(changed);
        if !mail && !calendar && self.generation > 0 {
            return;
        }
        self.generation += 1;
        if mail || self.generation == 1 {
            self.account = imap::Account::new(
                &global_str(&self.global, "imapServer"),
                &global_str(&self.global, "imapUser"),
                self.global
                    .get("imapPassword")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .map_err(|e| format!("{e:#}"));
            self.unread.clear();
            self.mail_error = None;
            self.mail_busy = false;
            self.check_mail(true);
        }
        if calendar || self.generation == 1 {
            self.events.clear();
            self.calendar_error = None;
            self.calendar_busy = false;
            self.load_calendars(true);
        }
        self.update_status();
        self.refresh_all();
    }

    /// Folders shown on mail keys.
    fn folders(&self) -> Vec<String> {
        let mut folders: Vec<String> = self
            .contexts
            .iter()
            .filter(|(_, i)| i.kind() == "mail")
            .map(|(_, i)| i.text_or("folder", "INBOX"))
            .collect();
        folders.sort();
        folders.dedup();
        folders
    }

    /// `force` also checks without mail keys (connection test).
    fn check_mail(&mut self, force: bool) {
        let Ok(account) = self.account.clone() else {
            return;
        };
        let mut folders = self.folders();
        if folders.is_empty() && force {
            folders.push("INBOX".into());
        }
        if self.mail_busy || folders.is_empty() {
            return;
        }
        self.mail_busy = true;
        let (tx, generation) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            tx.send(Msg::Mail(generation, account.unseen(&folders).await))
                .ok();
        });
    }

    fn load_calendars(&mut self, force: bool) {
        let calendars = self.calendars();
        if self.calendar_busy || calendars.is_empty() || !(force || self.contexts.any("event")) {
            return;
        }
        self.calendar_busy = true;
        let (tx, generation) = (self.tx.clone(), self.generation);
        tokio::spawn(async move {
            let http = reqwest::Client::builder()
                .user_agent(concat!(
                    "OpenDeckN3-MailKalender/",
                    env!("CARGO_PKG_VERSION")
                ))
                .timeout(Duration::from_secs(20))
                .build();
            let mut results = Vec::new();
            for (name, url) in calendars {
                let result = match &http {
                    Ok(http) => fetch_calendar(http, &url).await,
                    Err(err) => Err(anyhow::anyhow!("{err}")),
                };
                results.push((name, result));
            }
            tx.send(Msg::Calendars(generation, results)).ok();
        });
    }

    fn on_mail(&mut self, generation: u64, result: anyhow::Result<HashMap<String, u32>>) {
        if generation != self.generation {
            return;
        }
        self.mail_busy = false;
        match result {
            Ok(counts) => {
                self.unread.extend(counts);
                self.mail_error = None;
            }
            Err(err) => {
                let err = format!("{err:#}");
                if self.mail_error.as_ref() != Some(&err) {
                    self.host.log(format!("Mail: {err}"));
                }
                self.mail_error = Some(err);
            }
        }
        self.update_status();
        self.refresh_all();
    }

    fn on_calendars(
        &mut self,
        generation: u64,
        results: Vec<(String, anyhow::Result<Vec<ics::Event>>)>,
    ) {
        if generation != self.generation {
            return;
        }
        self.calendar_busy = false;
        let mut errors = Vec::new();
        for (name, result) in results {
            match result {
                Ok(events) => {
                    self.events.insert(name, events);
                }
                Err(err) => errors.push(format!("{name}: {err:#}")),
            }
        }
        let error = (!errors.is_empty()).then(|| errors.join("; "));
        if error.is_some() && error != self.calendar_error {
            self.host.log(format!("Kalender: {}", errors.join("; ")));
        }
        self.calendar_error = error;
        self.update_status();
        self.refresh_all();
    }

    /// Next event that has not ended, from the chosen calendar or all.
    fn next_event(&self, inst: &Instance) -> Option<&ics::Event> {
        let calendar = inst.text("calendar");
        let now = Utc::now();
        self.events
            .iter()
            .filter(|(name, _)| calendar.is_empty() || **name == calendar)
            .flat_map(|(_, events)| events)
            .filter(|e| e.end > now || e.end == e.start && e.start >= now)
            .min_by_key(|e| e.start)
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        match inst.kind() {
            "mail" => {
                if self.account.is_err() || self.mail_error.is_some() {
                    return (0, "offline".into());
                }
                match self.unread.get(&inst.text_or("folder", "INBOX")) {
                    Some(0) | None => (0, String::new()),
                    Some(n) if *n > 999 => (1, "999+".into()),
                    Some(n) => (1, n.to_string()),
                }
            }
            "event" => {
                let Some(event) = self.next_event(inst) else {
                    if self.calendars().is_empty() || self.events.is_empty() {
                        let failed = self.calendar_error.is_some() || self.calendars().is_empty();
                        return (0, if failed { "offline" } else { "" }.into());
                    }
                    return (0, "frei".into());
                };
                let now = Utc::now();
                let lead = chrono::Duration::minutes(inst.number("lead", 5.0).max(0.0) as i64);
                let soon = u16::from(event.start - now <= lead);
                // The key fits ~10 characters: alternate time and title.
                let show_summary = inst.flag("showSummary", true)
                    && !event.summary.is_empty()
                    && self.ticks % 2 == 1;
                let title = if show_summary {
                    event.summary.clone()
                } else {
                    ics::when(event, now)
                };
                (soon, title)
            }
            _ => (0, String::new()),
        }
    }

    fn refresh(&mut self, context: &str) {
        if let Some(inst) = self.contexts.get(context) {
            let (state, title) = self.appearance(inst);
            self.contexts.show(&self.host, context, state, &title);
        }
    }

    fn refresh_all(&mut self) {
        for context in self.contexts.ids() {
            self.refresh(&context);
        }
    }

    fn on_tick(&mut self) {
        self.ticks += 1;
        if self.ticks.is_multiple_of(MAIL_EVERY) {
            self.check_mail(false);
        }
        if self.ticks.is_multiple_of(CALENDAR_EVERY) {
            self.load_calendars(false);
        }
        self.refresh_all();
    }

    fn open(&self, url: &str) {
        self.host
            .send(json!({ "event": "openUrl", "payload": { "url": url } }));
    }

    fn press(&mut self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        match inst.kind() {
            "mail" => {
                self.open(&inst.text_or("url", MAIL_URL));
                self.check_mail(false);
            }
            "event" => {
                let link = self.next_event(&inst).and_then(|e| e.link.clone());
                self.open(&link.unwrap_or_else(|| inst.text_or("url", CALENDAR_URL)));
            }
            _ => {}
        }
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            let placed = self
                .contexts
                .get(&context)
                .map(|i| (i.kind().to_owned(), i.text_or("folder", "INBOX")));
            match placed {
                Some((kind, folder)) if kind == "mail" && !self.unread.contains_key(&folder) => {
                    self.check_mail(false)
                }
                Some((kind, _)) if kind == "event" && self.events.is_empty() => {
                    self.load_calendars(false)
                }
                _ => {}
            }
            self.refresh(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "sendToPlugin" => {
                let req = &msg["payload"];
                let result = match req["request"].as_str() {
                    Some("calendars") => {
                        let mut list = vec![(String::new(), "Alle Kalender".to_owned())];
                        list.extend(
                            self.calendars()
                                .into_iter()
                                .map(|(name, _)| (name.clone(), name)),
                        );
                        Ok(list)
                    }
                    other => Err(anyhow::anyhow!("unbekannte Anfrage {other:?}")),
                };
                self.host.reply_options(req, result);
            }
            "keyDown" => self.press(&context),
            _ => {}
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Mail(generation, result) => self.on_mail(generation, result),
            Msg::Calendars(generation, results) => self.on_calendars(generation, results),
        }
    }
}

/// `Name = Adresse` per line (or just the address). URLs may contain `=`,
/// so a line is only split when the part before `=` is no URL.
fn parse_calendars(text: &str) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, url) = match line.split_once('=') {
            Some((name, url)) if !name.contains("://") => (name.trim(), url.trim()),
            _ => (line, line),
        };
        let name = if name == url {
            format!("Kalender {}", list.len() + 1)
        } else {
            name.to_owned()
        };
        list.push((name, url.to_owned()));
    }
    list
}

async fn fetch_calendar(http: &reqwest::Client, url: &str) -> anyhow::Result<Vec<ics::Event>> {
    // Calendar apps hand out `webcal://` links; they are plain HTTPS.
    let url = match url.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    };
    anyhow::ensure!(
        url.starts_with("https://") || url.starts_with("http://"),
        "keine gültige Adresse (https://…)"
    );
    let res = http
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("nicht erreichbar: {e}"))?;
    match res.status().as_u16() {
        200..=299 => {}
        401 | 403 | 404 => anyhow::bail!(
            "Adresse abgelehnt (HTTP {}) – geheime iCal-Adresse prüfen",
            res.status().as_u16()
        ),
        code => anyhow::bail!("HTTP {code}"),
    }
    let text = res.text().await?;
    anyhow::ensure!(
        text.contains("BEGIN:VCALENDAR"),
        "keine iCal-Datei – die geheime Adresse im iCal-Format verwenden"
    );
    Ok(ics::upcoming(
        &text,
        Utc::now(),
        chrono::Duration::days(HORIZON_DAYS),
    ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx);
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.handle(Msg::Host(msg)),
                None => break,
            },
            Some(msg) = rx.recv() => plugin.handle(msg),
            _ = tick.tick() => plugin.on_tick(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One event starting in two hours, with a Teams link.
    fn feed() -> &'static str {
        let start = Utc::now() + chrono::Duration::hours(2);
        let (start, end) = (
            start.format("%Y%m%dT%H%M%SZ"),
            (start + chrono::Duration::minutes(15)).format("%Y%m%dT%H%M%SZ"),
        );
        format!(
            "BEGIN:VCALENDAR
BEGIN:VEVENT
UID:1
SUMMARY:Standup
DTSTART:{start}
DTEND:{end}
LOCATION:https://teams.microsoft.com/l/meetup-join/x
END:VEVENT
END:VCALENDAR
"
        )
        .leak()
    }

    /// Serves `body` for every request.
    async fn fake_http(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let _ = sock.read(&mut buf).await.unwrap();
                let res = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/calendar\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(res.as_bytes()).await.unwrap();
            }
        });
        format!("http://{addr}/basic.ics")
    }

    fn sent(rx: &mut mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn calendar_list() {
        assert_eq!(
            parse_calendars(
                "Arbeit = https://x/a.ics?k=1\n# aus\nhttps://y/b.ics?token=2\n\nwebcal://z/c"
            ),
            [
                ("Arbeit".to_owned(), "https://x/a.ics?k=1".to_owned()),
                (
                    "Kalender 2".to_owned(),
                    "https://y/b.ics?token=2".to_owned()
                ),
                ("Kalender 3".to_owned(), "webcal://z/c".to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn next_event_key_shows_time_and_opens_meeting() {
        let url = fake_http(feed()).await;
        let (host, mut host_rx) = Host::channel("de.opendeckn3.mailcalendar");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Plugin::new(host, tx);
        p.on_host(json!({ "event": "willAppear", "context": "e", "action": "de.opendeckn3.mailcalendar.event",
            "payload": { "controller": "Keypad", "settings": { "showSummary": "no" } } }));
        p.on_host(json!({ "event": "didReceiveGlobalSettings",
            "payload": { "settings": { "calendars": format!("Arbeit = {url}") } } }));
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        p.handle(msg);
        assert!(p.calendar_error.is_none(), "{:?}", p.calendar_error);
        let out = sent(&mut host_rx);
        let event = &p.events["Arbeit"][0];
        assert_eq!(event.summary, "Standup");
        let title = ics::when(event, Utc::now());
        assert!(title.contains(':'), "{title}");
        assert!(
            out.iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == title),
            "{out:?}"
        );
        assert!(out.iter().any(|m| m["event"] == "setGlobalSettings"
            && m["payload"]["status"] == "Kalender: 1 geladen"));
        p.on_host(json!({ "event": "keyDown", "context": "e" }));
        let out = sent(&mut host_rx);
        assert_eq!(
            out.last().unwrap()["payload"]["url"],
            "https://teams.microsoft.com/l/meetup-join/x"
        );
    }

    #[test]
    fn mail_key_shows_unread_count() {
        let (host, mut host_rx) = Host::channel("de.opendeckn3.mailcalendar");
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut p = Plugin::new(host, tx);
        p.on_host(json!({ "event": "willAppear", "context": "m", "action": "de.opendeckn3.mailcalendar.mail",
            "payload": { "controller": "Keypad", "settings": {} } }));
        p.account = imap::Account::new("imap.example.com", "u", "p").map_err(|e| e.to_string());
        p.on_mail(0, Ok(HashMap::from([("INBOX".to_owned(), 4)])));
        let out = sent(&mut host_rx);
        assert!(
            out.iter()
                .any(|m| m["event"] == "setState" && m["payload"]["state"] == 1)
        );
        assert!(
            out.iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == "4")
        );
        p.on_mail(0, Err(anyhow::anyhow!("Anmeldung abgelehnt")));
        let out = sent(&mut host_rx);
        assert!(
            out.iter()
                .any(|m| m["event"] == "setTitle" && m["payload"]["title"] == "offline")
        );
    }
}

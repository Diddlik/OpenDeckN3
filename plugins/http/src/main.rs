#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 HTTP plugin: sends HTTP requests (webhooks of n8n, Zapier,
//! IFTTT, own servers), turns the rotary knob into requests and shows values
//! from JSON responses (see `docs/PLUGIN_HTTP.md`).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::Context;
use n3_plugin_sdk::{Contexts, Host, Instance};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const TICK: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(30);
const MIN_INTERVAL_SECS: f64 = 5.0;
const MAX_INTERVAL_SECS: f64 = 86_400.0;
const ERROR_TITLE: &str = "–";
const PLAIN_TEXT_CHARS: usize = 12;

/// One HTTP request, built from the settings of an action.
struct Call {
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
    content_type: String,
}

enum Msg {
    Host(Value),
    /// Result of a value poll; `context` is the key that polls.
    Poll(String, anyhow::Result<String>),
}

/// Poll state of a value key. `next: None` means: poll as soon as possible.
#[derive(Default)]
struct Poll {
    next: Option<Instant>,
    busy: bool,
    error: Option<String>,
}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Msg>,
    http: reqwest::Client,
    contexts: Contexts,
    polls: HashMap<String, Poll>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Msg>) -> anyhow::Result<Self> {
        Ok(Self {
            host,
            tx,
            http: reqwest::Client::builder()
                .user_agent(concat!("OpenDeckN3-HTTP/", env!("CARGO_PKG_VERSION")))
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            contexts: Contexts::default(),
            polls: HashMap::new(),
        })
    }

    fn on_host(&mut self, msg: Value) {
        if let Some(context) = self.contexts.apply(&msg) {
            self.polls.remove(&context);
            return;
        }
        let context = msg["context"].as_str().unwrap_or_default().to_owned();
        match msg["event"].as_str().unwrap_or_default() {
            "keyDown" | "dialDown" => self.press(&context),
            "dialRotate" => self.rotate(&context, msg["payload"]["ticks"].as_i64().unwrap_or(0)),
            _ => {}
        }
    }

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        match inst.kind() {
            "request" => self.request(context, inst, 0),
            "poll" => self.open_url(context, &inst),
            _ => {}
        }
    }

    fn rotate(&self, context: &str, ticks: i64) {
        let Some(inst) = self.contexts.get(context).cloned() else {
            return;
        };
        if inst.kind() == "request" && ticks != 0 {
            self.request(context, inst, ticks);
        }
    }

    fn open_url(&self, context: &str, inst: &Instance) {
        let url = inst.text("openUrl");
        if url.is_empty() {
            return;
        }
        match check_url(&url) {
            Ok(url) => self
                .host
                .send(json!({ "event": "openUrl", "payload": { "url": url } })),
            Err(err) => {
                self.host.alert(context);
                self.host.log(format!("Link nicht geöffnet: {err:#}"));
            }
        }
    }

    /// Runs in its own task so a slow server does not block the main loop.
    fn request(&self, context: &str, inst: Instance, ticks: i64) {
        let (host, http, context) = (self.host.clone(), self.http.clone(), context.to_owned());
        let pointer = inst.text("showResponse");
        tokio::spawn(async move {
            let result: anyhow::Result<String> =
                async { send(&http, request_call(&inst, ticks)?).await }.await;
            match result {
                Ok(body) => {
                    host.ok(&context);
                    if !pointer.is_empty() {
                        match pointer_title(&body, &pointer) {
                            Ok(title) => host.set_title(&context, &title),
                            Err(err) => host.log(format!("Antwort nicht anzeigbar: {err:#}")),
                        }
                    }
                }
                Err(err) => {
                    host.alert(&context);
                    host.log(format!("HTTP-Anfrage fehlgeschlagen: {err:#}"));
                }
            }
        });
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        self.polls
            .retain(|context, _| self.contexts.get(context).is_some());
        for (context, inst) in self.contexts.iter() {
            if inst.kind() != "poll" {
                continue;
            }
            let poll = self.polls.entry(context.clone()).or_default();
            if poll.busy || poll.next.is_some_and(|next| next > now) {
                continue;
            }
            poll.busy = true;
            self.fetch(context, inst.clone());
        }
    }

    fn fetch(&self, context: &str, inst: Instance) {
        let (tx, http, context) = (self.tx.clone(), self.http.clone(), context.to_owned());
        tokio::spawn(async move {
            let result: anyhow::Result<String> = async {
                let body = send(&http, poll_call(&inst)?).await?;
                poll_title(&inst, &body)
            }
            .await;
            tx.send(Msg::Poll(context, result)).ok();
        });
    }

    fn on_poll(&mut self, context: &str, result: anyhow::Result<String>) {
        let Some(poll) = self.polls.get_mut(context) else {
            return;
        };
        poll.busy = false;
        let interval = self
            .contexts
            .get(context)
            .map_or(DEFAULT_INTERVAL, poll_interval);
        poll.next = Some(Instant::now() + interval);
        let title = match result {
            Ok(title) => {
                poll.error = None;
                title
            }
            Err(err) => {
                let err = format!("{err:#}");
                // Log once per change, not on every failed poll.
                if poll.error.as_ref() != Some(&err) {
                    self.host.log(format!("Wert nicht abrufbar: {err}"));
                }
                poll.error = Some(err);
                ERROR_TITLE.to_owned()
            }
        };
        self.contexts.show(&self.host, context, 0, &title);
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(msg) => self.on_host(msg),
            Msg::Poll(context, result) => self.on_poll(&context, result),
        }
    }
}

/// NaN (from a text setting such as "NaN") is not a valid duration and falls back to the default.
fn poll_interval(inst: &Instance) -> Duration {
    let secs = inst
        .number("interval", DEFAULT_INTERVAL.as_secs_f64())
        .clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS);
    Duration::try_from_secs_f64(secs).unwrap_or(DEFAULT_INTERVAL)
}

fn request_call(inst: &Instance, ticks: i64) -> anyhow::Result<Call> {
    Ok(Call {
        method: reqwest::Method::from_bytes(inst.text_or("method", "POST").as_bytes())
            .context("unbekannte Methode")?,
        url: check_url(&fill_ticks(&inst.text("url"), ticks))?.to_owned(),
        headers: parse_headers(&inst.text("headers"))?,
        body: fill_ticks(inst.settings["body"].as_str().unwrap_or_default(), ticks),
        content_type: inst.text_or("contentType", "application/json"),
    })
}

fn poll_call(inst: &Instance) -> anyhow::Result<Call> {
    Ok(Call {
        method: reqwest::Method::GET,
        url: check_url(&inst.text("url"))?.to_owned(),
        headers: parse_headers(&inst.text("headers"))?,
        body: String::new(),
        content_type: String::new(),
    })
}

async fn send(http: &reqwest::Client, call: Call) -> anyhow::Result<String> {
    let mut req = http.request(call.method, call.url.as_str());
    if !call.body.is_empty()
        && !call
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        req = req.header("Content-Type", call.content_type);
    }
    for (name, value) in call.headers {
        req = req.header(name, value);
    }
    if !call.body.is_empty() {
        req = req.body(call.body);
    }
    let res = req.send().await.context("keine Verbindung zum Ziel")?;
    let status = res.status();
    let body = res.text().await.context("Antwort nicht lesbar")?;
    anyhow::ensure!(status.is_success(), "HTTP {}", status.as_u16());
    Ok(body)
}

/// Only web addresses: the plugin must not open or read local files.
fn check_url(url: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(
        url.starts_with("http://") || url.starts_with("https://"),
        "Adresse muss mit http:// oder https:// beginnen"
    );
    Ok(url)
}

/// One `Name: Value` per line; blank lines are skipped.
fn parse_headers(text: &str) -> anyhow::Result<Vec<(String, String)>> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| -> anyhow::Result<(String, String)> {
            let (name, value) = line
                .split_once(':')
                .with_context(|| format!("Header ohne Doppelpunkt: {line}"))?;
            anyhow::ensure!(!name.trim().is_empty(), "Header ohne Namen: {line}");
            Ok((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// `{{ticks}}` becomes the signed step count, e.g. `2` or `-1`; no `+`, which
/// a query string would read as a space.
fn fill_ticks(text: &str, ticks: i64) -> String {
    text.replace("{{ticks}}", &ticks.to_string())
}

/// Title of a poll without JSON pointer: the start of the raw response.
fn plain_text(body: &str) -> String {
    body.trim().chars().take(PLAIN_TEXT_CHARS).collect()
}

/// Value at a JSON pointer as text; strings without quotes, other JSON as is.
fn pointer_title(body: &str, pointer: &str) -> anyhow::Result<String> {
    let json: Value = serde_json::from_str(body).context("Antwort ist kein JSON")?;
    let value = json
        .pointer(pointer)
        .with_context(|| format!("{pointer} fehlt in der Antwort"))?;
    Ok(match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn poll_title(inst: &Instance, body: &str) -> anyhow::Result<String> {
    let pointer = inst.text("pointer");
    let value = if pointer.is_empty() {
        plain_text(body)
    } else {
        pointer_title(body, &pointer)?
    };
    Ok(format!(
        "{}{value}{}",
        inst.text("prefix"),
        inst.text("suffix")
    ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut plugin = Plugin::new(host, tx)?;
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

    #[test]
    fn header_lines() {
        assert_eq!(
            parse_headers("\n X-Key: abc \n\nAuthorization: Bearer a:b\n").unwrap(),
            vec![
                ("X-Key".to_owned(), "abc".to_owned()),
                ("Authorization".to_owned(), "Bearer a:b".to_owned()),
            ]
        );
        assert!(parse_headers("kein Doppelpunkt").is_err());
        assert!(parse_headers(": ohne Name").is_err());
    }

    #[test]
    fn ticks_replace_placeholder() {
        assert_eq!(fill_ticks("/vol?step={{ticks}}", 2), "/vol?step=2");
        assert_eq!(fill_ticks("{\"d\": {{ticks}}}", -3), "{\"d\": -3}");
        assert_eq!(fill_ticks("ohne Platzhalter", 1), "ohne Platzhalter");
    }

    #[test]
    fn pointer_output() {
        let body = r#"{"s":"on","n":21.5,"b":true,"o":{"x":1}}"#;
        assert_eq!(pointer_title(body, "/s").unwrap(), "on");
        assert_eq!(pointer_title(body, "/n").unwrap(), "21.5");
        assert_eq!(pointer_title(body, "/b").unwrap(), "true");
        assert_eq!(pointer_title(body, "/o").unwrap(), r#"{"x":1}"#);
        assert!(pointer_title(body, "/fehlt").is_err());
        assert!(pointer_title("kein json", "/s").is_err());
        assert_eq!(plain_text(" 0123456789abcdef "), "0123456789ab");
    }

    #[test]
    fn url_scheme() {
        assert!(check_url("https://example.com/hook").is_ok());
        assert!(check_url("http://localhost:5678/webhook/x").is_ok());
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("ftp://example.com").is_err());
        assert!(check_url("").is_err());
    }
}

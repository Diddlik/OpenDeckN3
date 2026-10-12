#![cfg_attr(windows, windows_subsystem = "windows")]
//! OpenDeckN3 GitHub plugin: CI status of a branch, open pull requests and
//! unread notifications over the GitHub REST API (see `docs/PLUGIN_GITHUB.md`).

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use n3_plugin_sdk::{Contexts, Host, Instance, global_str};
use reqwest::Url;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

const API: &str = "https://api.github.com";
const GITHUB: &str = "https://github.com/";
/// Requests of one URL run at most this often; all keys showing the URL share the result.
const POLL: Duration = Duration::from_secs(60);
const DEFAULT_FILTER: &str = "review-requested:@me";
const DEFAULT_BRANCH: &str = "main";
const REPO_HINT: &str = "Repo als Besitzer/Name eintragen, z. B. Diddlik/OpenDeckN3";
/// Run states that mean "still going" and "failed"; every other completed run counts as success.
const RUNNING: [&str; 5] = ["queued", "in_progress", "waiting", "requested", "pending"];
const FAILED: [&str; 5] = [
    "failure",
    "cancelled",
    "timed_out",
    "startup_failure",
    "action_required",
];

/// Result of a request, reduced to what the keys show.
enum Data {
    /// Newest run of the branch; `None` when the branch has no runs.
    Run(Option<Run>),
    Count(u64),
}

struct Run {
    state: u16,
    name: String,
    url: String,
}

/// Last answer for one request URL.
#[derive(Default)]
struct Feed {
    last: Option<Instant>,
    busy: bool,
    /// `None` after a failed request.
    data: Option<Data>,
}

/// Answer of a request: token generation, request URL, result.
type Answer = (u64, String, anyhow::Result<Data>);

/// The token was rejected (HTTP 401).
#[derive(Debug)]
struct BadToken;

impl std::fmt::Display for BadToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token ungültig")
    }
}

impl std::error::Error for BadToken {}

/// The rate limit is used up; holds the reset time as a Unix timestamp if GitHub sent it.
#[derive(Debug)]
struct Limited(Option<u64>);

impl std::fmt::Display for Limited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitHub-Rate-Limit erreicht")
    }
}

impl std::error::Error for Limited {}

struct Plugin {
    host: Host,
    tx: mpsc::UnboundedSender<Answer>,
    http: reqwest::Client,
    contexts: Contexts,
    global: Map<String, Value>,
    /// Bumped when the token changes; answers to an older token are dropped.
    generation: u64,
    /// The first global settings message only loads values.
    loaded: bool,
    feeds: HashMap<String, Feed>,
    paused_until: Option<Instant>,
}

impl Plugin {
    fn new(host: Host, tx: mpsc::UnboundedSender<Answer>, http: reqwest::Client) -> Self {
        Self {
            host,
            tx,
            http,
            contexts: Contexts::default(),
            global: Map::new(),
            generation: 0,
            loaded: false,
            feeds: HashMap::new(),
            paused_until: None,
        }
    }

    fn token(&self) -> String {
        global_str(&self.global, "token")
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.global.insert("status".into(), json!(status.into()));
        self.host.save_global(&self.global);
    }

    fn refresh_status(&mut self) {
        let status = if self.token().is_empty() {
            "Kein Token – nur öffentliche Repos, 60 Abfragen pro Stunde"
        } else {
            "Token gesetzt"
        };
        self.set_status(status);
    }

    fn open_url(&self, url: &str) {
        self.host
            .send(json!({ "event": "openUrl", "payload": { "url": url } }));
    }

    fn on_global(&mut self, settings: Value) {
        let previous = std::mem::replace(
            &mut self.global,
            settings.as_object().cloned().unwrap_or_default(),
        );
        if !std::mem::replace(&mut self.loaded, true) {
            self.refresh_status();
            return;
        }
        if previous.get("token") != self.global.get("token") {
            self.generation += 1;
            self.feeds.clear();
            self.paused_until = None;
            self.refresh_status();
            self.refresh_all();
        }
    }

    fn on_answer(&mut self, generation: u64, url: String, result: anyhow::Result<Data>) {
        if generation != self.generation {
            return;
        }
        let Some(feed) = self.feeds.get_mut(&url) else {
            return;
        };
        feed.busy = false;
        match result {
            Ok(data) => feed.data = Some(data),
            Err(err) => {
                feed.data = None;
                self.on_failure(&err);
            }
        }
        self.refresh_all();
    }

    /// Errors that change the state are shown in the status; all errors go to the log.
    fn on_failure(&mut self, err: &anyhow::Error) {
        if err.is::<BadToken>() {
            self.set_status("Token ungültig");
        } else if let Some(Limited(reset)) = err.downcast_ref::<Limited>() {
            self.paused_until = Some(Instant::now() + rate_limit_pause(*reset, unix_now()));
            self.set_status("Rate-Limit erreicht – Abfragen pausiert");
        }
        self.host.log(format!("{err:#}"));
    }

    /// Starts the requests of all visible keys that are due. Keys with the same URL share one request.
    fn poll(&mut self) {
        if self
            .paused_until
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let token = self.token();
        let now = Instant::now();
        let mut wanted = HashSet::new();
        let mut jobs = Vec::new();
        for (_, inst) in self.contexts.iter() {
            let kind = inst.kind();
            if kind == "notifications" && token.is_empty() {
                continue;
            }
            let Ok(url) = request(kind, inst) else {
                continue;
            };
            let key = url.to_string();
            let feed = self.feeds.entry(key.clone()).or_default();
            if !feed.busy && feed.last.is_none_or(|at| now.duration_since(at) >= POLL) {
                feed.busy = true;
                feed.last = Some(now);
                jobs.push((kind.to_owned(), url));
            }
            wanted.insert(key);
        }
        self.feeds.retain(|key, _| wanted.contains(key));
        for (kind, url) in jobs {
            let (http, tx, token, generation) = (
                self.http.clone(),
                self.tx.clone(),
                token.clone(),
                self.generation,
            );
            tokio::spawn(async move {
                let result = fetch(&http, &token, &kind, &url).await;
                tx.send((generation, url.to_string(), result)).ok();
            });
        }
    }

    fn data(&self, inst: &Instance) -> Option<&Data> {
        let url = request(inst.kind(), inst).ok()?;
        self.feeds.get(url.as_str())?.data.as_ref()
    }

    fn press(&self, context: &str) {
        let Some(inst) = self.contexts.get(context) else {
            return;
        };
        let target: anyhow::Result<String> = match inst.kind() {
            "ci" => self.ci_page(inst),
            "prs" => search_query(&inst.text_or("filter", DEFAULT_FILTER), &inst.text("repo"))
                .and_then(|query| {
                    let url = Url::parse_with_params("https://github.com/pulls", &[("q", query)])?;
                    Ok(url.to_string())
                }),
            "notifications" => Ok("https://github.com/notifications".to_owned()),
            _ => return,
        };
        match target {
            Ok(url) => self.open_url(&url),
            Err(err) => {
                self.host.alert(context);
                self.host.log(format!("{err:#}"));
            }
        }
    }

    /// The newest run on the key, or the Actions page of the repo while no run is known.
    fn ci_page(&self, inst: &Instance) -> anyhow::Result<String> {
        let repo = inst.text("repo");
        anyhow::ensure!(valid_repo(&repo), REPO_HINT);
        if let Some(Data::Run(Some(run))) = self.data(inst)
            && run.url.starts_with(GITHUB)
        {
            return Ok(run.url.clone());
        }
        Ok(format!("{GITHUB}{repo}/actions"))
    }

    fn appearance(&self, inst: &Instance) -> (u16, String) {
        match (inst.kind(), self.data(inst)) {
            ("notifications", _) if self.token().is_empty() => (0, "Token nötig".into()),
            ("ci", Some(Data::Run(Some(run)))) => (run.state, run.name.clone()),
            ("ci", Some(Data::Run(None))) => (0, "Kein Lauf".into()),
            ("prs", Some(Data::Count(count))) => (0, count.to_string()),
            ("notifications", Some(Data::Count(count))) => {
                (u16::from(*count > 0), count.to_string())
            }
            _ => (0, "–".into()),
        }
    }

    fn refresh_all(&mut self) {
        for context in self.contexts.ids() {
            if let Some(inst) = self.contexts.get(&context) {
                let (state, title) = self.appearance(inst);
                self.contexts.show(&self.host, &context, state, &title);
            }
        }
    }

    fn on_host(&mut self, msg: Value) {
        if self.contexts.apply(&msg).is_some() {
            self.refresh_all();
            return;
        }
        match msg["event"].as_str().unwrap_or_default() {
            "didReceiveGlobalSettings" => self.on_global(msg["payload"]["settings"].clone()),
            "keyDown" => self.press(msg["context"].as_str().unwrap_or_default()),
            _ => {}
        }
    }
}

/// The request a key shows. An error means the key's settings are incomplete or wrong.
fn request(kind: &str, inst: &Instance) -> anyhow::Result<Url> {
    match kind {
        "ci" => runs_url(
            &inst.text("repo"),
            &inst.text_or("branch", DEFAULT_BRANCH),
            &inst.text("workflow"),
        ),
        "prs" => {
            let query = search_query(&inst.text_or("filter", DEFAULT_FILTER), &inst.text("repo"))?;
            Ok(Url::parse_with_params(
                &format!("{API}/search/issues"),
                &[("q", query)],
            )?)
        }
        _ => Ok(Url::parse_with_params(
            &format!("{API}/notifications"),
            &[("per_page", "50")],
        )?),
    }
}

fn runs_url(repo: &str, branch: &str, workflow: &str) -> anyhow::Result<Url> {
    anyhow::ensure!(valid_repo(repo), REPO_HINT);
    let base = if workflow.is_empty() {
        format!("{API}/repos/{repo}/actions/runs")
    } else {
        anyhow::ensure!(valid_name(workflow), "Workflow-Datei wie ci.yml eintragen");
        format!("{API}/repos/{repo}/actions/workflows/{workflow}/runs")
    };
    Ok(Url::parse_with_params(
        &base,
        &[("branch", branch), ("per_page", "1")],
    )?)
}

fn search_query(filter: &str, repo: &str) -> anyhow::Result<String> {
    match filter {
        "author:@me" | "review-requested:@me" => Ok(format!("is:pr is:open {filter}")),
        "repo" => {
            anyhow::ensure!(valid_repo(repo), REPO_HINT);
            Ok(format!("is:pr is:open repo:{repo}"))
        }
        _ => anyhow::bail!("Unbekannter Filter für Pull Requests"),
    }
}

/// `owner/name`, both parts GitHub-style names.
fn valid_repo(repo: &str) -> bool {
    repo.split_once('/')
        .is_some_and(|(owner, name)| valid_name(owner) && valid_name(name))
}

/// Starts with a letter or digit, then only letters, digits, `-`, `_` and `.`. Keeps `/` and `..` out of URLs.
fn valid_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn run_state(status: &str, conclusion: Option<&str>) -> u16 {
    if RUNNING.contains(&status) {
        1
    } else if status == "completed" && conclusion.is_some_and(|c| FAILED.contains(&c)) {
        2
    } else {
        0
    }
}

fn parse_run(run: &Value) -> Option<Run> {
    Some(Run {
        state: run_state(run["status"].as_str()?, run["conclusion"].as_str()),
        name: run["name"].as_str().unwrap_or_default().to_owned(),
        url: run["html_url"].as_str().unwrap_or_default().to_owned(),
    })
}

fn parse(kind: &str, body: &Value) -> Data {
    match kind {
        "ci" => Data::Run(parse_run(&body["workflow_runs"][0])),
        "prs" => Data::Count(body["total_count"].as_u64().unwrap_or(0)),
        _ => Data::Count(body.as_array().map_or(0, |items| items.len() as u64)),
    }
}

/// Only 403 and 429 with `x-ratelimit-remaining: 0` mean the rate limit is used up.
fn is_rate_limited(code: u16, remaining: Option<&str>) -> bool {
    matches!(code, 403 | 429) && remaining == Some("0")
}

/// Time until the rate limit resets; without a reset time, wait one poll interval.
fn rate_limit_pause(reset: Option<u64>, now: u64) -> Duration {
    reset.map_or(POLL, |at| {
        Duration::from_secs(at.saturating_sub(now).max(1))
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn header(res: &reqwest::Response, name: &str) -> Option<String> {
    res.headers().get(name)?.to_str().ok().map(str::to_owned)
}

async fn fetch(http: &reqwest::Client, token: &str, kind: &str, url: &Url) -> anyhow::Result<Data> {
    let mut req = http
        .get(url.clone())
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if !token.is_empty() {
        req = req.bearer_auth(token);
    }
    let res = req.send().await.context("GitHub nicht erreichbar")?;
    let code = res.status().as_u16();
    let remaining = header(&res, "x-ratelimit-remaining");
    if is_rate_limited(code, remaining.as_deref()) {
        let reset = header(&res, "x-ratelimit-reset").and_then(|r| r.parse().ok());
        return Err(Limited(reset).into());
    }
    match code {
        401 => return Err(BadToken.into()),
        404 => anyhow::bail!("nicht gefunden – Repo, Branch oder Workflow prüfen"),
        _ => {}
    }
    anyhow::ensure!(
        res.status().is_success(),
        "GitHub antwortet mit HTTP {code}"
    );
    let body: Value = res.json().await.context("Antwort nicht lesbar")?;
    Ok(parse(kind, &body))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (host, mut host_rx) = n3_plugin_sdk::connect().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Answer>();
    let http = reqwest::Client::builder()
        .user_agent("OpenDeckN3")
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut plugin = Plugin::new(host, tx, http);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            msg = host_rx.recv() => match msg {
                Some(msg) => plugin.on_host(msg),
                None => break,
            },
            Some((generation, url, result)) = rx.recv() => plugin.on_answer(generation, url, result),
            _ = tick.tick() => plugin.poll(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_follows_status_and_conclusion() {
        assert_eq!(run_state("queued", None), 1);
        assert_eq!(run_state("in_progress", None), 1);
        assert_eq!(run_state("completed", Some("success")), 0);
        assert_eq!(run_state("completed", Some("skipped")), 0);
        assert_eq!(run_state("completed", Some("failure")), 2);
        assert_eq!(run_state("completed", Some("timed_out")), 2);
        assert_eq!(run_state("completed", Some("startup_failure")), 2);
    }

    #[test]
    fn repo_must_be_owner_and_name() {
        assert!(valid_repo("Diddlik/OpenDeckN3"));
        assert!(valid_repo("a.b/c_d-e"));
        for bad in [
            "Diddlik", "a/", "/b", "a/b/c", "../x", "x/..", "a b/c", "a/b?x=1", "",
        ] {
            assert!(!valid_repo(bad), "{bad} must be rejected");
        }
    }

    #[test]
    fn search_query_names_the_filter() {
        assert_eq!(
            search_query("author:@me", "").unwrap(),
            "is:pr is:open author:@me"
        );
        assert_eq!(
            search_query("repo", "Diddlik/OpenDeckN3").unwrap(),
            "is:pr is:open repo:Diddlik/OpenDeckN3"
        );
        assert!(search_query("repo", "kaputt").is_err());
        assert!(search_query("alles", "").is_err());
    }

    #[test]
    fn search_url_encodes_the_query() {
        let query = search_query("review-requested:@me", "").unwrap();
        let url = Url::parse_with_params(&format!("{API}/search/issues"), &[("q", query)]).unwrap();
        assert_eq!(
            url.query(),
            Some("q=is%3Apr+is%3Aopen+review-requested%3A%40me")
        );
    }

    #[test]
    fn runs_url_uses_workflow_path_and_encodes_branch() {
        let url = runs_url("Diddlik/OpenDeckN3", "feature/x", "").unwrap();
        assert_eq!(url.path(), "/repos/Diddlik/OpenDeckN3/actions/runs");
        assert_eq!(url.query(), Some("branch=feature%2Fx&per_page=1"));
        let url = runs_url("Diddlik/OpenDeckN3", "main", "ci.yml").unwrap();
        assert_eq!(
            url.path(),
            "/repos/Diddlik/OpenDeckN3/actions/workflows/ci.yml/runs"
        );
        assert!(runs_url("Diddlik/OpenDeckN3", "main", "../x").is_err());
        assert!(runs_url("kaputt", "main", "").is_err());
    }

    #[test]
    fn identical_keys_share_one_request_url() {
        let mut contexts = Contexts::default();
        for context in ["a", "b"] {
            contexts.apply(&json!({
                "event": "willAppear", "context": context, "action": "p.ci",
                "payload": { "settings": { "repo": "Diddlik/OpenDeckN3" } }
            }));
        }
        let urls: Vec<String> = ["a", "b"]
            .iter()
            .map(|c| request("ci", contexts.get(c).unwrap()).unwrap().to_string())
            .collect();
        assert_eq!(urls[0], urls[1]);
    }

    #[test]
    fn rate_limit_is_only_403_or_429_with_no_remaining_calls() {
        assert!(is_rate_limited(403, Some("0")));
        assert!(is_rate_limited(429, Some("0")));
        assert!(!is_rate_limited(403, Some("12")));
        assert!(!is_rate_limited(403, None));
        assert!(!is_rate_limited(500, Some("0")));
    }

    #[test]
    fn rate_limit_pauses_until_reset() {
        assert_eq!(rate_limit_pause(Some(1000), 940), Duration::from_secs(60));
        assert_eq!(rate_limit_pause(Some(900), 940), Duration::from_secs(1));
        assert_eq!(rate_limit_pause(None, 940), POLL);
    }

    #[test]
    fn answers_are_read_from_the_body() {
        let runs = json!({ "workflow_runs": [{
            "status": "completed", "conclusion": "failure", "name": "CI",
            "html_url": "https://github.com/o/n/actions/runs/1"
        }] });
        match parse("ci", &runs) {
            Data::Run(Some(run)) => {
                assert_eq!(run.state, 2);
                assert_eq!(run.name, "CI");
                assert_eq!(run.url, "https://github.com/o/n/actions/runs/1");
            }
            _ => panic!("run expected"),
        }
        assert!(matches!(
            parse("ci", &json!({ "workflow_runs": [] })),
            Data::Run(None)
        ));
        assert!(matches!(
            parse("prs", &json!({ "total_count": 3 })),
            Data::Count(3)
        ));
        assert!(matches!(
            parse("notifications", &json!([{}, {}])),
            Data::Count(2)
        ));
    }
}

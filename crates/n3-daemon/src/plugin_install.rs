//! Installing plugins from archives (`.streamDeckPlugin` / `.zip`) and from
//! GitHub releases, plus the plugin catalog (`registry.json`).
//!
//! Downloading and unpacking run in background tasks; the result is a plugin
//! directory in `<plugins>/.staging/…` that the main loop then activates via
//! [`ApiCommand::ActivatePlugin`] (state changes stay in `App`).

use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, bail, ensure};
use base64::Engine;
use n3_plugin::PluginHost;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::api::{ApiCommand, ApiRequest};

/// Catalog of the OpenDeckN3 project (entries are added by pull request).
pub const DEFAULT_REGISTRY: &str =
    "https://raw.githubusercontent.com/Diddlik/OpenDeckN3/main/plugins/registry.json";

const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
const MAX_UNPACKED: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const CACHE_FOR: Duration = Duration::from_secs(10 * 60);
const ARCHIVE_EXTENSIONS: [&str; 3] = [".streamdeckplugin", ".zip", ".n3plugin"];

/// One entry of a `registry.json`.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// `owner/name` of the GitHub repository whose releases carry the plugin.
    pub repo: String,
    /// Exact asset name; otherwise the best `.streamDeckPlugin`/`.zip` asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Registry {
    #[serde(default)]
    plugins: Vec<RegistryEntry>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// `sha256:<hex>`, provided by GitHub for newer uploads.
    #[serde(default)]
    digest: Option<String>,
}

/// The release asset chosen for installation.
#[derive(Clone, Debug)]
struct Release {
    version: String,
    page: String,
    published: Option<String>,
    asset: GhAsset,
}

impl Release {
    fn to_json(&self) -> Value {
        json!({
            "version": self.version,
            "page": self.page,
            "published": self.published,
            "asset": self.asset.name,
            "size": self.asset.size,
        })
    }
}

type Cache = Option<(Instant, Vec<(RegistryEntry, Result<Release, String>)>)>;

/// Handles the plugin commands that need network or disk time.
#[derive(Clone)]
pub struct Installer {
    plugins_dir: PathBuf,
    registries: Arc<Vec<String>>,
    host: PluginHost,
    api: mpsc::Sender<ApiRequest>,
    http: reqwest::Client,
    cache: Arc<Mutex<Cache>>,
}

impl Installer {
    pub fn new(
        plugins_dir: PathBuf,
        registries: Vec<String>,
        host: PluginHost,
        api: mpsc::Sender<ApiRequest>,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("OpenDeckN3/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()?;
        // Leftovers of an interrupted installation.
        std::fs::remove_dir_all(plugins_dir.join(".staging")).ok();
        Ok(Self {
            plugins_dir,
            registries: Arc::new(registries),
            host,
            api,
            http,
            cache: Default::default(),
        })
    }

    /// `true` for commands this type answers (in the background).
    pub fn handles(command: &ApiCommand) -> bool {
        matches!(
            command,
            ApiCommand::InstallPlugin { .. }
                | ApiCommand::PluginStore { .. }
                | ApiCommand::UpdatePlugins
        )
    }

    pub fn spawn(&self, command: ApiCommand, reply: oneshot::Sender<Result<Value, String>>) {
        let this = self.clone();
        tokio::spawn(async move {
            let result = this.handle(command).await.map_err(|e| format!("{e:#}"));
            if let Err(err) = &result {
                tracing::warn!(%err, "plugin installation failed");
            }
            reply.send(result).ok();
        });
    }

    async fn handle(&self, command: ApiCommand) -> anyhow::Result<Value> {
        match command {
            ApiCommand::PluginStore { refresh } => self.store(refresh).await,
            ApiCommand::UpdatePlugins => self.update_all().await,
            ApiCommand::InstallPlugin {
                path,
                data,
                repo,
                asset,
            } => {
                let (archive, source) = match (path, data, repo) {
                    (Some(path), None, None) => {
                        let bytes = tokio::fs::read(&path)
                            .await
                            .with_context(|| format!("Datei {} nicht lesbar", path.display()))?;
                        (bytes, json!({ "file": path }))
                    }
                    (None, Some(data), None) => {
                        // Plain base64 or a data URL.
                        let b64 = data.rsplit_once(',').map_or(data.as_str(), |(_, b)| b);
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(b64.trim())
                            .context("Dateiinhalt ist kein gültiges Base64")?;
                        (bytes, json!({ "upload": true }))
                    }
                    (None, None, Some(repo)) => {
                        let repo = parse_repo(&repo)?;
                        let release = self.latest_release(&repo, asset.as_deref()).await?;
                        let bytes = self.download(&repo, &release.asset).await?;
                        (bytes, json!({ "repo": repo, "release": release.to_json() }))
                    }
                    _ => bail!("genau eines von path, data oder repo angeben"),
                };
                let mut result = self.install_archive(archive).await?;
                result["source"] = source;
                Ok(result)
            }
            _ => bail!("not an installer command"),
        }
    }

    /// Installs every catalog plugin with a newer release; reports
    /// `{updated: [{uuid, name, from, to}], failed: [{uuid, name, error}]}`.
    async fn update_all(&self) -> anyhow::Result<Value> {
        let store = self.store(true).await?;
        let (mut updated, mut failed) = (Vec::new(), Vec::new());
        let due = store["plugins"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["update"] == true);
        for plugin in due {
            let info = json!({ "uuid": plugin["uuid"], "name": plugin["name"] });
            let result = async {
                let repo = parse_repo(plugin["repo"].as_str().unwrap_or_default())?;
                let release = self.latest_release(&repo, plugin["asset"].as_str()).await?;
                let bytes = self.download(&repo, &release.asset).await?;
                self.install_archive(bytes).await
            };
            match result.await {
                Ok(_) => {
                    tracing::info!(plugin = %plugin["uuid"], to = %plugin["latest"]["version"], "plugin updated");
                    let mut info = info;
                    info["from"] = plugin["installed"].clone();
                    info["to"] = plugin["latest"]["version"].clone();
                    updated.push(info);
                }
                Err(err) => {
                    tracing::warn!(plugin = %plugin["uuid"], "plugin update failed: {err:#}");
                    let mut info = info;
                    info["error"] = json!(format!("{err:#}"));
                    failed.push(info);
                }
            }
        }
        Ok(json!({ "updated": updated, "failed": failed }))
    }

    /// Background job: shortly after start and then every few hours, update
    /// plugins if the user enabled it (`autoUpdatePlugins`).
    pub fn spawn_auto_update(&self, shutdown: CancellationToken) {
        const FIRST_CHECK: Duration = Duration::from_secs(60);
        const INTERVAL: Duration = Duration::from_secs(6 * 3600);
        let this = self.clone();
        tokio::spawn(async move {
            let mut wait = FIRST_CHECK;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = shutdown.cancelled() => return,
                }
                wait = INTERVAL;
                let enabled = this
                    .call(ApiCommand::GetAppSettings)
                    .await
                    .is_ok_and(|s| s["autoUpdatePlugins"] != false);
                if !enabled {
                    continue;
                }
                match this.update_all().await {
                    Ok(report) if report["updated"].as_array().is_some_and(|u| !u.is_empty()) => {
                        this.call(ApiCommand::ReportPluginUpdates {
                            plugins: report["updated"].clone(),
                        })
                        .await
                        .ok();
                    }
                    Ok(_) => {}
                    Err(err) => tracing::warn!("automatic plugin update failed: {err:#}"),
                }
            }
        });
    }

    /// Unpacks into a staging directory and lets the main loop activate it.
    async fn install_archive(&self, archive: Vec<u8>) -> anyhow::Result<Value> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let staging = self
            .plugins_dir
            .join(".staging")
            .join(format!("{}-{n}", std::process::id()));
        let unpack_into = staging.clone();
        let root = tokio::task::spawn_blocking(move || unpack(&archive, &unpack_into)).await?;
        let result = match root {
            Ok(root) => self.call(ApiCommand::ActivatePlugin { staged: root }).await,
            Err(err) => Err(err),
        };
        tokio::fs::remove_dir_all(&staging).await.ok();
        // Only succeeds when no other installation is running.
        tokio::fs::remove_dir(self.plugins_dir.join(".staging"))
            .await
            .ok();
        result
    }

    async fn call(&self, command: ApiCommand) -> anyhow::Result<Value> {
        let (tx, rx) = oneshot::channel();
        self.api
            .send((command, tx))
            .await
            .map_err(|_| anyhow::anyhow!("service is shutting down"))?;
        rx.await?.map_err(|e| anyhow::anyhow!(e))
    }

    // ---------------------------------------------------------------------
    // Catalog
    // ---------------------------------------------------------------------

    async fn store(&self, refresh: bool) -> anyhow::Result<Value> {
        let mut errors = Vec::new();
        let entries = {
            let mut cache = self.cache.lock().await;
            match cache.as_ref() {
                Some((at, entries)) if !refresh && at.elapsed() < CACHE_FOR => entries.clone(),
                _ => {
                    let mut list: Vec<RegistryEntry> = Vec::new();
                    for source in self.registries.iter() {
                        match self.fetch_registry(source).await {
                            Ok(found) => {
                                for entry in found {
                                    if !list.iter().any(|e| e.uuid == entry.uuid) {
                                        list.push(entry);
                                    }
                                }
                            }
                            Err(err) => errors.push(format!("{source}: {err:#}")),
                        }
                    }
                    let lookups = list.iter().map(|e| async {
                        let release = match parse_repo(&e.repo) {
                            Ok(repo) => self.latest_release(&repo, e.asset.as_deref()).await,
                            Err(err) => Err(err),
                        };
                        (e.clone(), release.map_err(|e| format!("{e:#}")))
                    });
                    let entries = futures_util::future::join_all(lookups).await;
                    if errors.is_empty() {
                        *cache = Some((Instant::now(), entries.clone()));
                    }
                    entries
                }
            }
        };

        let mut plugins = Vec::new();
        for (entry, release) in entries {
            let installed = self.host.get(&entry.uuid).await;
            let installed_version = installed.as_ref().map(|p| p.manifest.version.clone());
            let update = match (&installed_version, &release) {
                (Some(have), Ok(rel)) => is_newer(&rel.version, have),
                _ => false,
            };
            let mut v = serde_json::to_value(&entry)?;
            v["installed"] = json!(installed_version);
            v["update"] = json!(update);
            match release {
                Ok(rel) => v["latest"] = rel.to_json(),
                Err(err) => v["error"] = json!(err),
            }
            plugins.push(v);
        }
        Ok(json!({
            "plugins": plugins,
            "registries": *self.registries,
            "errors": errors,
        }))
    }

    async fn fetch_registry(&self, source: &str) -> anyhow::Result<Vec<RegistryEntry>> {
        let text = if source.starts_with("https://") || source.starts_with("http://") {
            let res = self.http.get(source).send().await?;
            ensure!(res.status().is_success(), "HTTP {}", res.status());
            res.text().await?
        } else {
            tokio::fs::read_to_string(source).await?
        };
        let registry: Registry = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .context("registry.json ungültig")?;
        Ok(registry.plugins)
    }

    // ---------------------------------------------------------------------
    // GitHub
    // ---------------------------------------------------------------------

    async fn latest_release(&self, repo: &str, asset: Option<&str>) -> anyhow::Result<Release> {
        let url = format!("https://api.github.com/repos/{repo}/releases?per_page=20");
        let res = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .context("GitHub nicht erreichbar")?;
        let limited = res
            .headers()
            .get("x-ratelimit-remaining")
            .is_some_and(|v| v == "0");
        match res.status().as_u16() {
            200 => {}
            404 => bail!("Repository {repo} nicht gefunden"),
            429 => bail!("GitHub-Abfragelimit erreicht – bitte später erneut versuchen"),
            403 if limited => bail!("GitHub-Abfragelimit erreicht – bitte später erneut versuchen"),
            code => bail!("GitHub antwortet mit HTTP {code}"),
        }
        let releases: Vec<GhRelease> = serde_json::from_slice(&res.bytes().await?)?;
        pick_release(&releases, asset, std::env::consts::OS).with_context(|| {
            format!("{repo}: kein Release mit Plugin-Datei (.streamDeckPlugin oder .zip)")
        })
    }

    async fn download(&self, repo: &str, asset: &GhAsset) -> anyhow::Result<Vec<u8>> {
        let prefix = format!("https://github.com/{repo}/releases/download/");
        ensure!(
            asset.browser_download_url.starts_with(&prefix),
            "unerwartete Download-Adresse {}",
            asset.browser_download_url
        );
        ensure!(asset.size <= MAX_DOWNLOAD, "Plugin-Datei ist zu groß");
        tracing::info!(url = %asset.browser_download_url, "downloading plugin");
        let mut res = self
            .http
            .get(&asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::with_capacity(asset.size as usize);
        while let Some(chunk) = res.chunk().await? {
            bytes.extend_from_slice(&chunk);
            ensure!(
                bytes.len() as u64 <= MAX_DOWNLOAD,
                "Plugin-Datei ist zu groß"
            );
        }
        if let Some(expected) = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
        {
            let actual = hex(&Sha256::digest(&bytes));
            ensure!(
                actual.eq_ignore_ascii_case(expected),
                "Prüfsumme stimmt nicht (Download beschädigt?)"
            );
        }
        Ok(bytes)
    }
}

/// `owner/name` from `owner/name`, `github.com/owner/name` or a GitHub URL.
pub fn parse_repo(input: &str) -> anyhow::Result<String> {
    let s = input.trim();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    let mut parts = s.split('/').filter(|p| !p.is_empty());
    let (Some(owner), Some(name)) = (parts.next(), parts.next()) else {
        bail!("GitHub-Repository als „besitzer/name“ angeben");
    };
    let name = name.strip_suffix(".git").unwrap_or(name);
    let valid = |p: &str| {
        !p.is_empty()
            && !p.starts_with('.')
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    ensure!(
        valid(owner) && valid(name),
        "ungültiges GitHub-Repository „{input}“"
    );
    Ok(format!("{owner}/{name}"))
}

/// Newest stable release with a plugin asset; a pre-release only if there is
/// no stable one. Releases come newest first from the API.
fn pick_release(releases: &[GhRelease], asset: Option<&str>, os: &str) -> Option<Release> {
    let mut candidates = releases.iter().filter(|r| !r.draft);
    let with_asset = |r: &GhRelease| {
        pick_asset(&r.assets, asset, os).map(|a| Release {
            version: r.tag_name.trim_start_matches('v').to_owned(),
            page: r.html_url.clone(),
            published: r.published_at.clone(),
            asset: a,
        })
    };
    candidates
        .clone()
        .filter(|r| !r.prerelease)
        .find_map(with_asset)
        .or_else(|| candidates.find_map(with_asset))
}

fn pick_asset(assets: &[GhAsset], wanted: Option<&str>, os: &str) -> Option<GhAsset> {
    if let Some(name) = wanted {
        return assets.iter().find(|a| a.name == name).cloned();
    }
    const OS_TOKENS: [(&str, &[&str]); 3] = [
        ("windows", &["windows", "win", "win64", "x64windows"]),
        ("linux", &["linux"]),
        ("macos", &["mac", "macos", "darwin", "osx"]),
    ];
    let tokens = |name: &str| -> Vec<String> {
        name.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .map(str::to_owned)
            .collect()
    };
    assets
        .iter()
        .filter(|a| {
            let lower = a.name.to_ascii_lowercase();
            ARCHIVE_EXTENSIONS.iter().any(|e| lower.ends_with(e))
        })
        .filter_map(|a| {
            let words = tokens(&a.name);
            let mut score = 0;
            for (name, list) in OS_TOKENS {
                if words.iter().any(|w| list.contains(&w.as_str())) {
                    if name == os {
                        score += 4;
                    } else {
                        return None; // built for another platform
                    }
                }
            }
            if a.name.to_ascii_lowercase().ends_with(".streamdeckplugin") {
                score += 1;
            }
            Some((score, a))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, a)| a.clone())
}

/// `1.2.0` vs. `1.10.0` etc.; falls back to "different = newer".
fn is_newer(latest: &str, installed: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v'));
    match (parse(latest), parse(installed)) {
        (Ok(a), Ok(b)) => a > b,
        _ => latest.trim().trim_start_matches('v') != installed.trim().trim_start_matches('v'),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks the plugin contained in `archive` below `staging` and returns the
/// plugin directory (the one holding the top-most `manifest.json`).
pub fn unpack(archive: &[u8], staging: &Path) -> anyhow::Result<PathBuf> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .context("keine gültige Plugin-Datei (ZIP / .streamDeckPlugin erwartet)")?;
    ensure!(zip.len() <= MAX_ENTRIES, "Archiv enthält zu viele Dateien");

    let mut root: Option<PathBuf> = None;
    for i in 0..zip.len() {
        let file = zip.by_index_raw(i)?;
        let Some(path) = safe_path(file.name()) else {
            continue;
        };
        if path.starts_with("__MACOSX") || path.file_name() != Some("manifest.json".as_ref()) {
            continue;
        }
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if root
            .as_ref()
            .is_none_or(|r| parent.components().count() < r.components().count())
        {
            root = Some(parent);
        }
    }
    let root = root.context("keine manifest.json im Archiv gefunden")?;
    let dir_name = root
        .file_name()
        .map_or_else(|| "plugin".into(), |n| n.to_owned());
    let out = staging.join(dir_name);
    std::fs::create_dir_all(&out)?;

    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let Some(path) = safe_path(file.name()) else {
            tracing::warn!(name = file.name(), "skipping unsafe path in plugin archive");
            continue;
        };
        let Ok(rel) = path.strip_prefix(&root) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = out.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer = std::fs::File::create(&target)
            .with_context(|| format!("{} anlegen", target.display()))?;
        // Do not trust the declared size (zip bombs).
        let limit = MAX_UNPACKED - total;
        let written = std::io::copy(&mut (&mut file).take(limit + 1), &mut writer)?;
        total += written;
        ensure!(total <= MAX_UNPACKED, "Archiv ist entpackt zu groß");
        #[cfg(unix)]
        if let Some(mode) = file.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o755)).ok();
        }
    }
    ensure!(
        out.join("manifest.json").is_file(),
        "manifest.json konnte nicht entpackt werden"
    );
    Ok(out)
}

/// Relative path of an archive entry, or `None` if it could escape the target
/// directory. Accepts `\` as separator (archives made with Windows PowerShell 5).
fn safe_path(name: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains([':', '\0']) => return None,
            p => path.push(p),
        }
    }
    (!path.as_os_str().is_empty() && !name.starts_with(['/', '\\'])).then_some(path)
}

/// Plugin UUIDs become directory names, so keep them boring.
pub fn validate_uuid(uuid: &str) -> anyhow::Result<()> {
    ensure!(
        !uuid.is_empty()
            && uuid.len() <= 128
            && !uuid.starts_with('.')
            && uuid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
        "ungültige Plugin-UUID „{uuid}“"
    );
    ensure!(
        uuid != n3_plugin::BUILTIN_PLUGIN,
        "Plugin-UUID ist reserviert"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in files {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const MANIFEST: &str = r#"{"UUID":"de.test.x","Name":"X","CodePath":"p.js","Actions":[]}"#;

    #[test]
    fn unpacks_stream_deck_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = zip_of(&[
            ("README.md", "hi"),
            ("de.test.x.sdPlugin/manifest.json", MANIFEST),
            ("de.test.x.sdPlugin/p.js", "//"),
            ("de.test.x.sdPlugin/sub/manifest.json", "{}"),
        ]);
        let dir = unpack(&archive, tmp.path()).unwrap();
        assert!(dir.ends_with("de.test.x.sdPlugin"));
        assert!(dir.join("p.js").is_file());
        assert!(dir.join("sub/manifest.json").is_file());
        assert!(!tmp.path().join("README.md").exists());
    }

    #[test]
    fn unpacks_flat_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = unpack(
            &zip_of(&[("manifest.json", MANIFEST), ("p.js", "//")]),
            tmp.path(),
        )
        .unwrap();
        assert!(dir.join("manifest.json").is_file() && dir.join("p.js").is_file());
    }

    #[test]
    fn rejects_unsafe_and_empty_archives() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = zip_of(&[("manifest.json", MANIFEST), ("../evil.txt", "x")]);
        let dir = unpack(&archive, &tmp.path().join("s")).unwrap();
        assert!(dir.join("manifest.json").is_file());
        assert!(!tmp.path().join("evil.txt").exists());
        assert!(unpack(&zip_of(&[("a.txt", "x")]), tmp.path()).is_err());
        let dir = unpack(
            &zip_of(&[("w.sdPlugin\\manifest.json", MANIFEST)]),
            &tmp.path().join("w"),
        )
        .unwrap();
        assert!(dir.ends_with("w.sdPlugin") && dir.join("manifest.json").is_file());
        assert!(safe_path("/etc/passwd").is_none() && safe_path("C:\\x").is_none());
        assert!(unpack(b"not a zip", tmp.path()).is_err());
    }

    #[test]
    fn repo_forms() {
        assert_eq!(
            parse_repo("Diddlik/OpenDeckN3").unwrap(),
            "Diddlik/OpenDeckN3"
        );
        assert_eq!(
            parse_repo("https://github.com/a-b/c.d.git/releases").unwrap(),
            "a-b/c.d"
        );
        assert!(parse_repo("nur-ein-teil").is_err());
        assert!(parse_repo("a/../b").is_err());
    }

    fn asset(name: &str) -> GhAsset {
        GhAsset {
            name: name.into(),
            size: 1,
            browser_download_url: String::new(),
            digest: None,
        }
    }

    #[test]
    fn picks_platform_asset_and_stable_release() {
        let assets = [
            asset("p-linux.zip"),
            asset("p-windows.zip"),
            asset("p.streamDeckPlugin"),
            asset("notes.txt"),
        ];
        assert_eq!(
            pick_asset(&assets, None, "windows").unwrap().name,
            "p-windows.zip"
        );
        assert_eq!(
            pick_asset(&assets, None, "macos").unwrap().name,
            "p.streamDeckPlugin"
        );
        // "darwin" contains "win" but is a separate token.
        assert!(pick_asset(&[asset("p-darwin.zip")], None, "windows").is_none());
        let rel = |tag: &str, pre: bool| GhRelease {
            tag_name: tag.into(),
            html_url: String::new(),
            draft: false,
            prerelease: pre,
            published_at: None,
            assets: vec![asset("p.zip")],
        };
        let releases = [rel("v2.0.0-beta", true), rel("v1.1.0", false)];
        assert_eq!(
            pick_release(&releases, None, "linux").unwrap().version,
            "1.1.0"
        );
        let only_pre = [rel("v2.0.0-beta", true)];
        assert_eq!(
            pick_release(&only_pre, None, "linux").unwrap().version,
            "2.0.0-beta"
        );
    }

    #[test]
    fn version_compare() {
        assert!(is_newer("1.10.0", "1.9.2"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(is_newer("2024-05", "2024-04"));
    }

    #[test]
    fn uuid_rules() {
        assert!(validate_uuid("de.opendeckn3.counter").is_ok());
        assert!(validate_uuid("../x").is_err());
        assert!(validate_uuid(".hidden").is_err());
        assert!(validate_uuid("opendeckn3.builtin").is_err());
    }
}

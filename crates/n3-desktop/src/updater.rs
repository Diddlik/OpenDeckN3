//! Update check against GitHub releases and installer download.
//!
//! Flow: [`check`] asks the GitHub API for releases newer than the running
//! version. [`download`] fetches the Windows installer of such a release and
//! verifies it against the release's `SHA256SUMS.txt`. The installer is then
//! started in passive update mode (`/P /UPDATE /R`, same as Tauri's updater):
//! it shows a progress window, keeps shortcuts and restarts the app.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ASSETS_API: &str = "https://api.github.com/repos/Diddlik/OpenDeckN3/releases/assets/";
const RELEASES_API: &str = "https://api.github.com/repos/Diddlik/OpenDeckN3/releases?per_page=30";
/// Downloads are only accepted from this repository's release assets.
const DOWNLOAD_PREFIX: &str = "https://github.com/Diddlik/OpenDeckN3/releases/download/";
const INSTALLER_SUFFIX: &str = "-windows-x64-setup.exe";
const CHECKSUMS: &str = "SHA256SUMS.txt";

#[derive(Debug, Deserialize)]
pub struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    #[serde(default)]
    id: u64,
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// What the UI shows and passes back to `update_install`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub name: String,
    pub notes: String,
    pub published_at: Option<String>,
    pub html_url: String,
    pub prerelease: bool,
    /// Windows installer; `None` if the release has none (then: manual download).
    pub installer_url: Option<String>,
    pub installer_name: Option<String>,
    pub installer_size: u64,
    pub checksums_url: Option<String>,
    /// Numeric IDs are resolved only within this repository, never to a caller-supplied API host.
    #[serde(default)]
    pub installer_asset_id: Option<u64>,
    #[serde(default)]
    pub checksums_asset_id: Option<u64>,
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("OpenDeckN3-Updater/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()?)
}

/// Picks the newest release that is newer than `current`.
pub fn select(
    releases: Vec<GhRelease>,
    current: &Version,
    include_prerelease: bool,
) -> Option<UpdateInfo> {
    releases
        .into_iter()
        .filter(|r| !r.draft && (include_prerelease || !r.prerelease))
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            (version > *current).then_some((version, r))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(version, r)| {
            let installer = r.assets.iter().find(|a| a.name.ends_with(INSTALLER_SUFFIX));
            let checksums = r.assets.iter().find(|a| a.name == CHECKSUMS);
            UpdateInfo {
                version: version.to_string(),
                name: r
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| r.tag_name.clone()),
                tag: r.tag_name,
                notes: r.body.unwrap_or_default(),
                published_at: r.published_at,
                html_url: r.html_url,
                prerelease: r.prerelease,
                installer_url: installer.map(|a| a.browser_download_url.clone()),
                installer_name: installer.map(|a| a.name.clone()),
                installer_size: installer.map_or(0, |a| a.size),
                checksums_url: checksums.map(|a| a.browser_download_url.clone()),
                installer_asset_id: installer.map(|a| a.id).filter(|id| *id > 0),
                checksums_asset_id: checksums.map(|a| a.id).filter(|id| *id > 0),
            }
        })
}

pub async fn check(
    current: &Version,
    include_prerelease: bool,
) -> anyhow::Result<Option<UpdateInfo>> {
    let response = http()?
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("GitHub ist nicht erreichbar")?;
    ensure!(
        response.status().is_success(),
        "GitHub antwortet mit {}",
        response.status()
    );
    let releases: Vec<GhRelease> = response
        .json()
        .await
        .context("Antwort von GitHub unlesbar")?;
    Ok(select(releases, current, include_prerelease))
}

/// Finds the hash for `file` in a `sha256sum`-style list.
pub fn expected_hash(checksums: &str, file: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// GitHub's browser download endpoint can fail independently of its asset API.
async fn fetch_response(
    url: &str,
    fallback_url: Option<&str>,
) -> anyhow::Result<reqwest::Response> {
    let client = http()?;
    match client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
    {
        Ok(response) => Ok(response),
        Err(first) => match fallback_url {
            Some(api_url) => client
                .get(api_url)
                .header("Accept", "application/octet-stream")
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .with_context(|| {
                    format!(
                        "Download fehlgeschlagen ({first}); GitHub-API ebenfalls nicht erreichbar"
                    )
                }),
            None => Err(first).context("Download fehlgeschlagen"),
        },
    }
}

/// Streams `url` into `dest`, returning the SHA-256 of the content.
pub async fn fetch_to_file(
    url: &str,
    fallback_url: Option<&str>,
    dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> anyhow::Result<String> {
    let mut response = fetch_response(url, fallback_url).await?;
    let total = response.content_length();
    let mut file = std::fs::File::create(dest)
        .with_context(|| format!("kann {} nicht schreiben", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    while let Some(chunk) = response.chunk().await.context("Download abgebrochen")? {
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush()?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Downloads and verifies the installer of `info`; returns its path.
pub async fn download(
    info: &UpdateInfo,
    progress: impl FnMut(u64, Option<u64>),
) -> anyhow::Result<PathBuf> {
    let (Some(url), Some(name)) = (&info.installer_url, &info.installer_name) else {
        bail!("Dieses Release enthält keinen Windows-Installer");
    };
    let Some(sums_url) = &info.checksums_url else {
        bail!(
            "Dieses Release enthält keine Prüfsummen ({CHECKSUMS}) – bitte manuell aktualisieren"
        );
    };
    ensure!(
        url.starts_with(DOWNLOAD_PREFIX) && sums_url.starts_with(DOWNLOAD_PREFIX),
        "unerwartete Download-Adresse"
    );
    ensure!(
        !name.contains(['/', '\\']) && name.ends_with(INSTALLER_SUFFIX),
        "unerwarteter Dateiname"
    );

    let sums_api = info
        .checksums_asset_id
        .filter(|id| *id > 0)
        .map(|id| format!("{ASSETS_API}{id}"));
    let installer_api = info
        .installer_asset_id
        .filter(|id| *id > 0)
        .map(|id| format!("{ASSETS_API}{id}"));
    let sums = fetch_response(sums_url, sums_api.as_deref())
        .await
        .context("Prüfsummen nicht abrufbar")?
        .text()
        .await?;
    let expected = expected_hash(&sums, name).context("Installer fehlt in den Prüfsummen")?;

    let dir = std::env::temp_dir().join("OpenDeckN3-update");
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(name);
    let actual = fetch_to_file(url, installer_api.as_deref(), &dest, progress).await?;
    if actual != expected {
        std::fs::remove_file(&dest).ok();
        bail!("Prüfsumme stimmt nicht – Download verworfen");
    }
    Ok(dest)
}

/// Starts the NSIS installer in passive update mode; the caller must exit right after.
pub fn launch_installer(path: &Path) -> anyhow::Result<()> {
    std::process::Command::new(path)
        .args(["/P", "/UPDATE", "/R"])
        .spawn()
        .context("Installer startet nicht")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            name: Some(format!("OpenDeckN3 {tag}")),
            body: Some("Neu: alles".into()),
            draft: false,
            prerelease,
            published_at: None,
            html_url: format!("https://github.com/Diddlik/OpenDeckN3/releases/tag/{tag}"),
            assets: assets
                .iter()
                .map(|n| GhAsset {
                    id: 42,
                    name: n.to_string(),
                    browser_download_url: format!("{DOWNLOAD_PREFIX}{tag}/{n}"),
                    size: 10,
                })
                .collect(),
        }
    }

    #[test]
    fn selects_newest_matching_release() {
        let current = Version::parse("0.1.0-alpha.2").unwrap();
        let list = || {
            vec![
                release("v0.1.0-alpha.1", true, &[]),
                release(
                    "v0.1.0-alpha.3",
                    true,
                    &["OpenDeckN3-0.1.0-alpha.3-windows-x64-setup.exe", CHECKSUMS],
                ),
                release("v0.1.0-alpha.2", true, &[]),
                release("not-a-version", false, &[]),
            ]
        };
        let update = select(list(), &current, true).unwrap();
        assert_eq!(update.version, "0.1.0-alpha.3");
        assert!(
            update
                .installer_url
                .as_ref()
                .unwrap()
                .ends_with("-setup.exe")
        );
        assert!(update.checksums_url.is_some());
        let mut legacy = serde_json::to_value(&update).unwrap();
        legacy.as_object_mut().unwrap().remove("installerAssetId");
        legacy.as_object_mut().unwrap().remove("checksumsAssetId");
        let legacy: UpdateInfo = serde_json::from_value(legacy).unwrap();
        assert!(legacy.installer_asset_id.is_none());
        assert!(legacy.checksums_asset_id.is_none());
        assert_eq!(update.installer_asset_id, Some(42));
        assert_eq!(update.checksums_asset_id, Some(42));

        assert!(
            select(list(), &current, false).is_none(),
            "prereleases skipped"
        );
        let newest = Version::parse("0.1.0-alpha.3").unwrap();
        assert!(
            select(list(), &newest, true).is_none(),
            "already up to date"
        );
        let stable = vec![
            release("v0.2.0", false, &[]),
            release("v0.3.0-beta.1", true, &[]),
        ];
        assert_eq!(select(stable, &current, false).unwrap().version, "0.2.0");
    }

    #[test]
    fn parses_checksum_lists() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  OpenDeckN3-1-windows-x64-setup.exe\n{}  other.zip\n",
            "b".repeat(64)
        );
        assert_eq!(
            expected_hash(&sums, "OpenDeckN3-1-windows-x64-setup.exe"),
            Some(h.clone())
        );
        assert_eq!(
            expected_hash(&format!("{} *x.exe", h.to_uppercase()), "x.exe"),
            Some(h)
        );
        assert_eq!(expected_hash("zz  x.exe", "x.exe"), None);
        assert_eq!(expected_hash(&sums, "missing.exe"), None);
    }

    #[tokio::test]
    async fn downloads_and_hashes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhallo")
                .await
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("f.bin");
        let mut seen = 0;
        let hash = fetch_to_file(&format!("http://{addr}/f"), None, &dest, |done, _| {
            seen = done
        })
        .await
        .unwrap();
        assert_eq!(seen, 5);
        assert_eq!(std::fs::read(&dest).unwrap(), b"hallo");
        // sha256("hallo")
        assert_eq!(
            hash,
            "d3751d33f9cd5049c4af2b462735457e4d3baf130bcbb87f389e349fbaeb20b9"
        );
    }

    #[tokio::test]
    async fn falls_back_after_gateway_timeout_and_verifies_download_content() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for path in ["/browser", "/api"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                assert!(request.starts_with(&format!("get {path} ")));
                let reply: &[u8] = if path == "/browser" {
                    b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    assert!(request.contains("accept: application/octet-stream"));
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhallo"
                };
                socket.write_all(reply).await.unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("installer.exe");
        let hash = fetch_to_file(
            &format!("http://{addr}/browser"),
            Some(&format!("http://{addr}/api")),
            &dest,
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(
            hash,
            "d3751d33f9cd5049c4af2b462735457e4d3baf130bcbb87f389e349fbaeb20b9"
        );
        assert_eq!(std::fs::read(dest).unwrap(), b"hallo");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn failed_download_does_not_create_an_installer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await.unwrap();
                socket.write_all(b"HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("installer.exe");
        let url = format!("http://{addr}/failed");
        assert!(
            fetch_to_file(&url, Some(&url), &dest, |_, _| {})
                .await
                .is_err()
        );
        assert!(!dest.exists());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn refuses_foreign_or_unverifiable_downloads() {
        let mut info = select(
            vec![release(
                "v9.0.0",
                false,
                &["OpenDeckN3-9.0.0-windows-x64-setup.exe"],
            )],
            &Version::new(0, 1, 0),
            false,
        )
        .unwrap();
        assert!(
            download(&info, |_, _| {})
                .await
                .unwrap_err()
                .to_string()
                .contains("Prüfsummen")
        );
        info.checksums_asset_id = Some(42);
        info.installer_asset_id = Some(43);
        info.checksums_url = Some("https://evil.example/SHA256SUMS.txt".into());
        assert!(
            download(&info, |_, _| {})
                .await
                .unwrap_err()
                .to_string()
                .contains("Adresse")
        );
    }

    /// Talks to the real GitHub API: `cargo test -p n3-desktop -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_check() {
        let update = check(&Version::new(0, 0, 1), true).await.unwrap();
        println!("{update:#?}");
        let update = update.expect("at least one release");
        let dest = download(&update, |_, _| {}).await.unwrap();
        assert!(std::fs::metadata(&dest).unwrap().len() > 0);
        println!("downloaded and verified {}", dest.display());
        std::fs::remove_file(dest).unwrap();
    }
}

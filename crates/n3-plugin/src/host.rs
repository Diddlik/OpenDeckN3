//! Plugin discovery, process management and the plugin WebSocket server.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::{Mutex, RwLock, mpsc},
};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    manifest::PluginManifest,
    protocol::{InboundEvent, RegisterEvent},
};

/// Something that happened on a plugin connection.
#[derive(Debug)]
pub enum PluginEvent {
    /// The plugin connected and registered; the daemon should now send it
    /// `deviceDidConnect` and `willAppear` for its instances.
    Registered,
    Disconnected,
    Inbound(InboundEvent),
}

/// A message received from a registered plugin.
#[derive(Debug)]
pub struct PluginMessage {
    pub plugin: String,
    pub event: PluginEvent,
}

/// A plugin found on disk.
#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub uuid: String,
    pub path: PathBuf,
    pub manifest: PluginManifest,
}

struct Inner {
    port: u16,
    plugins: RwLock<HashMap<String, InstalledPlugin>>,
    connections: RwLock<HashMap<String, mpsc::UnboundedSender<Message>>>,
    children: Mutex<HashMap<String, Child>>,
    inbound: mpsc::Sender<PluginMessage>,
    /// Application info passed to plugins on launch (set by `launch_all`).
    app_info: RwLock<Value>,
}

/// Cheaply clonable handle to the plugin host.
#[derive(Clone)]
pub struct PluginHost {
    inner: Arc<Inner>,
}

impl PluginHost {
    /// Binds the plugin WebSocket server on `127.0.0.1:port` and starts
    /// accepting plugin connections.
    pub async fn start(port: u16, inbound: mpsc::Sender<PluginMessage>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| format!("binding plugin port {port}"))?;
        let host = Self {
            inner: Arc::new(Inner {
                port: listener.local_addr()?.port(),
                plugins: Default::default(),
                connections: Default::default(),
                children: Default::default(),
                inbound,
                app_info: RwLock::new(Value::Null),
            }),
        };

        let accept_host = host.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        tokio::spawn(accept_host.clone().handle_connection(stream, addr));
                    }
                    Err(err) => tracing::error!(%err, "plugin accept failed"),
                }
            }
        });

        tracing::info!(port = host.inner.port, "plugin host listening");
        Ok(host)
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Reads all `manifest.json` files below `dir` (one plugin per sub-directory).
    pub async fn discover(&self, dir: &Path) -> anyhow::Result<()> {
        if !dir.exists() {
            tracing::info!(dir = %dir.display(), "plugin directory does not exist yet");
            return Ok(());
        }
        // Absolute paths, because plugins run with their own dir as cwd.
        // (`absolute` instead of `canonicalize`: no `\\?\` prefix on Windows.)
        let dir = std::path::absolute(dir)?;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if !path.join("manifest.json").is_file() {
                continue;
            }
            if let Err(err) = self.add(&path).await {
                tracing::warn!(dir = %path.display(), %err, "invalid plugin");
            }
        }
        Ok(())
    }

    /// Registers the plugin in `path` (a directory with `manifest.json`).
    /// Replaces an already known plugin with the same UUID.
    pub async fn add(&self, path: &Path) -> anyhow::Result<InstalledPlugin> {
        let path = std::path::absolute(path)?;
        let manifest = PluginManifest::read(&path)?;
        let uuid = plugin_uuid(&manifest, &path);
        anyhow::ensure!(uuid != crate::BUILTIN_PLUGIN, "plugin uses a reserved UUID");
        tracing::info!(%uuid, name = %manifest.name, "plugin discovered");
        let plugin = InstalledPlugin {
            uuid: uuid.clone(),
            path,
            manifest,
        };
        self.inner
            .plugins
            .write()
            .await
            .insert(uuid, plugin.clone());
        Ok(plugin)
    }

    /// Forgets a plugin (after `stop`). Returns what was registered.
    pub async fn remove(&self, uuid: &str) -> Option<InstalledPlugin> {
        self.inner.plugins.write().await.remove(uuid)
    }

    pub async fn get(&self, uuid: &str) -> Option<InstalledPlugin> {
        self.inner.plugins.read().await.get(uuid).cloned()
    }

    /// Stops the plugin's process and closes its connection.
    pub async fn stop(&self, uuid: &str) {
        if let Some(mut child) = self.inner.children.lock().await.remove(uuid) {
            tracing::info!(plugin = %uuid, "stopping plugin");
            child.kill().await.ok();
        }
        // Dropping the sender ends the writer task, which closes the socket.
        self.inner.connections.write().await.remove(uuid);
    }

    /// Starts the process of one registered plugin.
    pub async fn start_plugin(&self, uuid: &str) -> anyhow::Result<()> {
        let plugin = self.get(uuid).await.context("unknown plugin")?;
        let info = self.inner.app_info.read().await.clone();
        self.launch(&plugin, &info).await
    }

    pub async fn plugins(&self) -> Vec<InstalledPlugin> {
        let mut list: Vec<_> = self.inner.plugins.read().await.values().cloned().collect();
        list.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        list
    }

    pub async fn is_connected(&self, plugin: &str) -> bool {
        self.inner.connections.read().await.contains_key(plugin)
    }

    /// Starts the processes of all discovered plugins.
    pub async fn launch_all(&self, info: &Value) {
        *self.inner.app_info.write().await = info.clone();
        for plugin in self.plugins().await {
            if let Err(err) = self.launch(&plugin, info).await {
                tracing::error!(plugin = %plugin.uuid, %err, "failed to launch plugin");
            }
        }
    }

    async fn launch(&self, plugin: &InstalledPlugin, info: &Value) -> anyhow::Result<()> {
        let pid = self.spawn(plugin, info).await?;
        tokio::spawn(self.clone().supervise(plugin.uuid.clone(), pid));
        Ok(())
    }

    /// Restarts the plugin when its process ends with an error. Stops once
    /// the process was stopped or replaced, or after repeated crashes right
    /// after start, so a broken plugin does not restart forever.
    async fn supervise(self, uuid: String, mut pid: Option<u32>) {
        const POLL: Duration = Duration::from_secs(1);
        const STABLE: Duration = Duration::from_secs(60);
        const MAX_QUICK_CRASHES: u32 = 3;
        let mut started = Instant::now();
        let mut quick_crashes = 0;
        loop {
            tokio::time::sleep(POLL).await;
            let status = {
                let mut children = self.inner.children.lock().await;
                let Some(child) = children.get_mut(&uuid).filter(|c| c.id() == pid) else {
                    return;
                };
                match child.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(status)) => status,
                    Err(err) => {
                        tracing::warn!(plugin = %uuid, %err, "cannot query plugin process");
                        return;
                    }
                }
            };
            self.inner.children.lock().await.remove(&uuid);
            if status.success() {
                tracing::info!(plugin = %uuid, "plugin exited");
                return;
            }
            quick_crashes = if started.elapsed() < STABLE {
                quick_crashes + 1
            } else {
                1
            };
            if quick_crashes > MAX_QUICK_CRASHES {
                tracing::error!(plugin = %uuid, %status, "plugin keeps crashing, not restarting");
                return;
            }
            tracing::warn!(plugin = %uuid, %status, "plugin crashed, restarting");
            let (Some(plugin), info) = (
                self.get(&uuid).await,
                self.inner.app_info.read().await.clone(),
            ) else {
                return;
            };
            match self.spawn(&plugin, &info).await {
                Ok(new_pid) => {
                    pid = new_pid;
                    started = Instant::now();
                }
                Err(err) => {
                    tracing::error!(plugin = %uuid, %err, "failed to restart plugin");
                    return;
                }
            }
        }
    }

    /// Starts the plugin process and returns its PID.
    async fn spawn(&self, plugin: &InstalledPlugin, info: &Value) -> anyhow::Result<Option<u32>> {
        let code_path = plugin
            .manifest
            .code_path_for_current_os()
            .context("no CodePath for this platform")?;
        let full_path = plugin.path.join(code_path);

        let mut command = match full_path.extension().and_then(|e| e.to_str()) {
            Some("js" | "mjs" | "cjs") => {
                let mut c = Command::new("node");
                c.arg(&full_path);
                c
            }
            Some("py") => {
                let mut c = Command::new("python3");
                c.arg(&full_path);
                c
            }
            _ => Command::new(&full_path),
        };

        let info = json!({
            "plugin": { "uuid": plugin.uuid, "version": plugin.manifest.version },
            "application": info,
        });

        command
            .current_dir(&plugin.path)
            .args(["-port", &self.inner.port.to_string()])
            .args(["-pluginUUID", &plugin.uuid])
            .args(["-registerEvent", "registerPlugin"])
            .args(["-info", &info.to_string()])
            // Plugin output goes to our log instead of a console window.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Console programs (most plugins) would otherwise open a terminal window.
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let mut child = command
            .spawn()
            .with_context(|| format!("spawning {}", full_path.display()))?;
        let pid = child.id();
        tracing::info!(plugin = %plugin.uuid, pid, "plugin started");
        if let Some(out) = child.stdout.take() {
            tokio::spawn(log_lines(plugin.uuid.clone(), out));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(log_lines(plugin.uuid.clone(), err));
        }
        self.inner
            .children
            .lock()
            .await
            .insert(plugin.uuid.clone(), child);
        Ok(pid)
    }

    /// Stops all plugin processes.
    pub async fn shutdown(&self) {
        for (uuid, mut child) in self.inner.children.lock().await.drain() {
            tracing::info!(plugin = %uuid, "stopping plugin");
            child.kill().await.ok();
        }
    }

    /// Sends a JSON event to one plugin. Silently drops it if not connected.
    pub async fn send(&self, plugin: &str, event: &Value) {
        if let Some(tx) = self.inner.connections.read().await.get(plugin) {
            tx.send(Message::text(event.to_string())).ok();
        }
    }

    /// Sends a JSON event to all connected plugins.
    pub async fn broadcast(&self, event: &Value) {
        let text = event.to_string();
        for tx in self.inner.connections.read().await.values() {
            tx.send(Message::text(text.clone())).ok();
        }
    }

    async fn handle_connection(self, stream: TcpStream, addr: SocketAddr) {
        let ws = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(err) => {
                tracing::warn!(%addr, %err, "plugin websocket handshake failed");
                return;
            }
        };
        let (mut sink, mut stream) = ws.split();

        // The first text message must be the registration.
        let uuid = loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<RegisterEvent>(&text) {
                        Ok(RegisterEvent::RegisterPlugin { uuid }) => break uuid,
                        Err(err) => {
                            tracing::warn!(%addr, %err, "expected registerPlugin");
                            return;
                        }
                    }
                }
                Some(Ok(_)) => continue,
                _ => return,
            }
        };

        if !self.inner.plugins.read().await.contains_key(&uuid) {
            tracing::warn!(%uuid, "unknown plugin tried to register");
            return;
        }
        tracing::info!(%uuid, "plugin registered");

        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        let own = tx.clone();
        self.inner
            .connections
            .write()
            .await
            .insert(uuid.clone(), tx);

        self.notify(&uuid, PluginEvent::Registered).await;

        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        while let Some(msg) = stream.next().await {
            let text = match msg {
                Ok(Message::Text(text)) => text,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            match serde_json::from_str::<InboundEvent>(&text) {
                Ok(event) => self.notify(&uuid, PluginEvent::Inbound(event)).await,
                Err(err) => tracing::warn!(%uuid, %err, "undecodable plugin message"),
            }
        }

        {
            // A reinstalled plugin may already have registered a new connection.
            let mut connections = self.inner.connections.write().await;
            if connections.get(&uuid).is_some_and(|c| c.same_channel(&own)) {
                connections.remove(&uuid);
            }
        }
        writer.abort();
        self.notify(&uuid, PluginEvent::Disconnected).await;
        tracing::info!(%uuid, "plugin disconnected");
    }

    async fn notify(&self, plugin: &str, event: PluginEvent) {
        let msg = PluginMessage {
            plugin: plugin.to_owned(),
            event,
        };
        self.inner.inbound.send(msg).await.ok();
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Forwards a plugin's stdout/stderr line by line to the log.
async fn log_lines(plugin: String, stream: impl tokio::io::AsyncRead + Unpin) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            tracing::info!(target: "plugin", %plugin, "{line}");
        }
    }
}

/// UUID from the manifest, else the directory name without `.sdPlugin`.
pub fn plugin_uuid(manifest: &PluginManifest, dir: &Path) -> String {
    manifest.uuid.clone().unwrap_or_else(|| {
        let name = dir.file_name().unwrap_or_default().to_string_lossy();
        name.trim_end_matches(".sdPlugin").to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin that logs each start and crashes at once is restarted until
    /// the quick-crash limit stops it.
    #[tokio::test]
    async fn restarts_crashed_plugin_until_limit() {
        let dir = std::env::temp_dir().join(format!("n3-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{ "Name": "Crash", "UUID": "test.crash", "Version": "1.0.0",
                 "CodePath": "crash.js", "Actions": [] }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("crash.js"),
            "require('fs').appendFileSync('starts.txt', 'x'); process.exit(1);",
        )
        .unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let host = PluginHost::start(0, tx).await.unwrap();
        let plugin = host.add(&dir).await.unwrap();
        host.launch_all(&Value::Null).await;
        assert_eq!(plugin.uuid, "test.crash");

        let starts = || std::fs::read_to_string(dir.join("starts.txt")).unwrap_or_default();
        for _ in 0..30 {
            if starts().len() >= 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(starts().len(), 4, "1 start + 3 restarts");
        std::fs::remove_dir_all(&dir).ok();
    }
}

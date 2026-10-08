//! Plugin discovery, process management and the plugin WebSocket server.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
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
        let mut plugins = self.inner.plugins.write().await;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if !path.join("manifest.json").is_file() {
                continue;
            }
            match PluginManifest::read(&path) {
                Ok(manifest) => {
                    let uuid = manifest.uuid.clone().unwrap_or_else(|| {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        name.trim_end_matches(".sdPlugin").to_owned()
                    });
                    tracing::info!(%uuid, name = %manifest.name, "plugin discovered");
                    plugins.insert(
                        uuid.clone(),
                        InstalledPlugin {
                            uuid,
                            path,
                            manifest,
                        },
                    );
                }
                Err(err) => tracing::warn!(dir = %path.display(), %err, "invalid plugin"),
            }
        }
        Ok(())
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
        for plugin in self.plugins().await {
            if let Err(err) = self.launch(&plugin, info).await {
                tracing::error!(plugin = %plugin.uuid, %err, "failed to launch plugin");
            }
        }
    }

    async fn launch(&self, plugin: &InstalledPlugin, info: &Value) -> anyhow::Result<()> {
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
            .kill_on_drop(true);

        let child = command
            .spawn()
            .with_context(|| format!("spawning {}", full_path.display()))?;
        tracing::info!(plugin = %plugin.uuid, pid = child.id(), "plugin started");
        self.inner
            .children
            .lock()
            .await
            .insert(plugin.uuid.clone(), child);
        Ok(())
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

        self.inner.connections.write().await.remove(&uuid);
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

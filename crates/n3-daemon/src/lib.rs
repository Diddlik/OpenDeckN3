//! The OpenDeckN3 service as a library.
//!
//! Used by the `opendeckn3d` command-line binary and embedded by the desktop
//! app (`n3-desktop`). [`run`] owns the whole service until `shutdown` fires.

mod api;
mod app;
mod builtin;
mod render;
mod store;
mod ui_server;

use std::path::PathBuf;

use anyhow::Context;
use n3_plugin::PluginHost;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

pub use app::open_url;

pub const DEFAULT_PLUGIN_PORT: u16 = 57130;
pub const DEFAULT_API_PORT: u16 = 57131;
pub const DEFAULT_UI_PORT: u16 = 57132;

/// Everything needed to start the service.
#[derive(Clone, Debug)]
pub struct Options {
    /// Profiles and settings. See [`default_config_dir`].
    pub config_dir: PathBuf,
    /// Plugin directories in addition to `<config_dir>/plugins`.
    pub extra_plugin_dirs: Vec<PathBuf>,
    pub plugin_port: u16,
    pub api_port: u16,
    /// Port of the bundled web UI, `None` to not serve it.
    pub ui_port: Option<u16>,
    /// Additional browser origins allowed to use the UI API.
    pub allow_origins: Vec<String>,
    pub virtual_device: bool,
    pub hardware: bool,
}

impl Options {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir,
            extra_plugin_dirs: Vec::new(),
            plugin_port: DEFAULT_PLUGIN_PORT,
            api_port: DEFAULT_API_PORT,
            ui_port: Some(DEFAULT_UI_PORT),
            allow_origins: Vec::new(),
            virtual_device: false,
            hardware: true,
        }
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.config_dir.join("plugins")
    }
}

/// `~/.config/opendeckn3` on Linux, `%APPDATA%\opendeckn3` on Windows.
pub fn default_config_dir() -> anyhow::Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("no config directory on this platform")?
        .join("opendeckn3"))
}

/// `true` if another service instance already owns the UI API port.
pub fn is_running(api_port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", api_port)).is_err()
}

/// Runs the service until `shutdown` is cancelled.
pub async fn run(opts: Options, shutdown: CancellationToken) -> anyhow::Result<()> {
    let default_plugins_dir = opts.plugins_dir();
    std::fs::create_dir_all(&default_plugins_dir)?;
    let mut plugins_dirs = vec![default_plugins_dir];
    plugins_dirs.extend(opts.extra_plugin_dirs.iter().cloned());
    tracing::info!(config = %opts.config_dir.display(), ?plugins_dirs, "starting");

    let token = shutdown.child_token();
    let (device_tx, mut device_rx) = mpsc::channel(256);
    let (plugin_tx, mut plugin_rx) = mpsc::channel(256);
    let (api_tx, mut api_rx) = mpsc::channel::<api::ApiRequest>(64);
    let (ui_tx, _) = broadcast::channel(256);

    let plugins = PluginHost::start(opts.plugin_port, plugin_tx).await?;
    for dir in &plugins_dirs {
        if let Err(err) = plugins.discover(dir).await {
            tracing::warn!(dir = %dir.display(), %err, "cannot read plugin directory");
        }
    }
    let mut app = app::App::new(
        store::Store::new(&opts.config_dir),
        plugins.clone(),
        ui_tx.clone(),
        device_tx.clone(),
        token.clone(),
    );

    let mut origins = opts.allow_origins.clone();
    if let Some(ui_port) = opts.ui_port {
        for host in ["127.0.0.1", "localhost"] {
            origins.push(format!("http://{host}:{ui_port}"));
        }
    }
    // Desktop app (Tauri webview: Linux/macOS resp. Windows).
    origins.extend([
        "tauri://localhost".into(),
        "http://tauri.localhost".into(),
        "https://tauri.localhost".into(),
    ]);
    tokio::spawn(api::serve(
        opts.api_port,
        api::AllowedOrigins::new(origins),
        api_tx,
        ui_tx,
    ));
    if let Some(ui_port) = opts.ui_port {
        let api_port = opts.api_port;
        tokio::spawn(async move {
            if let Err(err) = ui_server::serve(ui_port, api_port).await {
                tracing::error!(%err, "UI server failed");
            }
        });
    }

    if opts.hardware {
        let (tx, token) = (device_tx.clone(), token.clone());
        tokio::spawn(async move {
            if let Err(err) = n3_driver::run_hid_watcher(tx, token).await {
                tracing::error!(%err, "HID watcher failed");
            }
        });
    }
    if opts.virtual_device {
        tokio::spawn(n3_driver::virtual_deck::run_virtual_device(
            device_tx.clone(),
            token.clone(),
        ));
    }
    drop(device_tx);

    plugins
        .launch_all(&json!({
            "platform": std::env::consts::OS,
            "version": env!("CARGO_PKG_VERSION"),
            "language": "de",
        }))
        .await;

    loop {
        tokio::select! {
            Some(event) = device_rx.recv() => app.on_device_event(event).await,
            Some(msg) = plugin_rx.recv() => app.on_plugin_message(msg).await,
            Some((command, reply)) = api_rx.recv() => {
                let result = app.on_api_command(command).await.map_err(|e| format!("{e:#}"));
                reply.send(result).ok();
            }
            _ = token.cancelled() => break,
        }
    }

    tracing::info!("shutting down");
    token.cancel();
    plugins.shutdown().await;
    Ok(())
}

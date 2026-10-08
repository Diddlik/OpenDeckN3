//! `opendeckn3d` – the OpenDeckN3 background service.

mod api;
mod app;
mod builtin;
mod render;
mod store;
mod ui_server;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use n3_plugin::PluginHost;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "opendeckn3d",
    version,
    about = "OpenDeckN3 – Stream-Controller-Dienst für den TreasLin N3"
)]
struct Args {
    /// Configuration directory (profiles, settings). Default: ~/.config/opendeckn3
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Additional plugin directory (repeatable). `<config-dir>/plugins` is always scanned.
    #[arg(long = "plugins-dir", value_name = "DIR")]
    plugins_dirs: Vec<PathBuf>,
    /// Port of the plugin WebSocket (Stream Deck SDK protocol).
    #[arg(long, default_value_t = 57130)]
    plugin_port: u16,
    /// Port of the UI WebSocket API.
    #[arg(long, default_value_t = 57131)]
    api_port: u16,
    /// Port of the bundled web UI (http://127.0.0.1:<port>/).
    #[arg(long, default_value_t = 57132)]
    ui_port: u16,
    /// Do not serve the bundled web UI.
    #[arg(long)]
    no_ui: bool,
    /// Additional browser origin allowed to use the UI API (e.g. a dev server).
    #[arg(long = "allow-origin", value_name = "ORIGIN")]
    allow_origins: Vec<String>,
    /// Open the web UI in the default browser once the service is ready.
    /// If the service is already running, only the browser is opened.
    #[arg(long)]
    open: bool,
    /// Add a virtual N3 for development without hardware.
    #[arg(long = "virtual")]
    virtual_device: bool,
    /// Do not look for USB/HID devices.
    #[arg(long)]
    no_hardware: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config_dir = match args.config_dir {
        Some(dir) => dir,
        None => dirs::config_dir()
            .context("no config directory on this platform")?
            .join("opendeckn3"),
    };
    let ui_url = format!("http://127.0.0.1:{}/", args.ui_port);

    // Single instance: a second start (e.g. via the Start menu) just opens the UI.
    if std::net::TcpListener::bind(("127.0.0.1", args.api_port)).is_err() {
        if args.open {
            tracing::info!("service already running, opening UI");
            return app::open_url(&ui_url);
        }
        anyhow::bail!(
            "port {} is in use – is opendeckn3d already running?",
            args.api_port
        );
    }

    let default_plugins_dir = config_dir.join("plugins");
    std::fs::create_dir_all(&default_plugins_dir)?;
    let mut plugins_dirs = vec![default_plugins_dir];
    plugins_dirs.extend(args.plugins_dirs.iter().cloned());
    tracing::info!(config = %config_dir.display(), ?plugins_dirs, "starting");

    let token = CancellationToken::new();
    let (device_tx, mut device_rx) = mpsc::channel(256);
    let (plugin_tx, mut plugin_rx) = mpsc::channel(256);
    let (api_tx, mut api_rx) = mpsc::channel::<api::ApiRequest>(64);
    let (ui_tx, _) = broadcast::channel(256);

    let plugins = PluginHost::start(args.plugin_port, plugin_tx).await?;
    for dir in &plugins_dirs {
        if let Err(err) = plugins.discover(dir).await {
            tracing::warn!(dir = %dir.display(), %err, "cannot read plugin directory");
        }
    }
    let mut app = app::App::new(
        store::Store::new(&config_dir),
        plugins.clone(),
        ui_tx.clone(),
        device_tx.clone(),
        token.clone(),
    );

    let mut origins = args.allow_origins.clone();
    for host in ["127.0.0.1", "localhost"] {
        origins.push(format!("http://{host}:{}", args.ui_port));
    }
    // Future desktop shell (Tauri).
    origins.extend(["tauri://localhost".into(), "http://tauri.localhost".into()]);
    tokio::spawn(api::serve(
        args.api_port,
        api::AllowedOrigins::new(origins),
        api_tx,
        ui_tx,
    ));
    if !args.no_ui {
        let (ui_port, api_port) = (args.ui_port, args.api_port);
        tokio::spawn(async move {
            if let Err(err) = ui_server::serve(ui_port, api_port).await {
                tracing::error!(%err, "UI server failed");
            }
        });
    }

    if !args.no_hardware {
        let (tx, token) = (device_tx.clone(), token.clone());
        tokio::spawn(async move {
            if let Err(err) = n3_driver::run_hid_watcher(tx, token).await {
                tracing::error!(%err, "HID watcher failed");
            }
        });
    }
    if args.virtual_device {
        tokio::spawn(n3_driver::virtual_deck::run_virtual_device(
            device_tx.clone(),
            token.clone(),
        ));
    }
    drop(device_tx);

    if !args.no_ui {
        println!(
            "\n  OpenDeckN3 läuft – Oberfläche: {ui_url}\n  Beenden: Strg+C oder dieses Fenster schließen.\n"
        );
        if args.open
            && let Err(err) = app::open_url(&ui_url)
        {
            tracing::warn!(%err, "cannot open browser");
        }
    }

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
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    tracing::info!("shutting down");
    token.cancel();
    plugins.shutdown().await;
    Ok(())
}

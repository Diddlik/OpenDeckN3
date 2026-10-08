//! `opendeckn3d` – the OpenDeckN3 background service.

mod api;
mod app;
mod builtin;
mod render;
mod store;

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
    /// Directory with installed plugins. Default: <config-dir>/plugins
    #[arg(long)]
    plugins_dir: Option<PathBuf>,
    /// Port of the plugin WebSocket (Stream Deck SDK protocol).
    #[arg(long, default_value_t = 57130)]
    plugin_port: u16,
    /// Port of the UI WebSocket API.
    #[arg(long, default_value_t = 57131)]
    api_port: u16,
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
    let plugins_dir = args
        .plugins_dir
        .unwrap_or_else(|| config_dir.join("plugins"));
    std::fs::create_dir_all(&plugins_dir)?;
    tracing::info!(config = %config_dir.display(), plugins = %plugins_dir.display(), "starting");

    let token = CancellationToken::new();
    let (device_tx, mut device_rx) = mpsc::channel(256);
    let (plugin_tx, mut plugin_rx) = mpsc::channel(256);
    let (api_tx, mut api_rx) = mpsc::channel::<api::ApiRequest>(64);
    let (ui_tx, _) = broadcast::channel(256);

    let plugins = PluginHost::start(args.plugin_port, plugin_tx).await?;
    plugins.discover(&plugins_dir).await?;
    let mut app = app::App::new(
        store::Store::new(&config_dir),
        plugins.clone(),
        ui_tx.clone(),
    );

    tokio::spawn(api::serve(args.api_port, api_tx, ui_tx));

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

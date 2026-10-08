//! `opendeckn3d` – the OpenDeckN3 service on the command line (Linux, headless,
//! development). The desktop app embeds the same service, see `n3-desktop`.

use std::path::PathBuf;

use clap::Parser;
use n3_daemon::{DEFAULT_API_PORT, DEFAULT_PLUGIN_PORT, DEFAULT_UI_PORT, Options};
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
    #[arg(long, default_value_t = DEFAULT_PLUGIN_PORT)]
    plugin_port: u16,
    /// Port of the UI WebSocket API.
    #[arg(long, default_value_t = DEFAULT_API_PORT)]
    api_port: u16,
    /// Port of the bundled web UI (http://127.0.0.1:<port>/).
    #[arg(long, default_value_t = DEFAULT_UI_PORT)]
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
        None => n3_daemon::default_config_dir()?,
    };
    let ui_url = format!("http://127.0.0.1:{}/", args.ui_port);

    // Single instance: a second start just opens the UI.
    if n3_daemon::is_running(args.api_port) {
        if args.open {
            tracing::info!("service already running, opening UI");
            return n3_daemon::open_url(&ui_url);
        }
        anyhow::bail!(
            "port {} is in use – is OpenDeckN3 already running?",
            args.api_port
        );
    }

    let opts = Options {
        extra_plugin_dirs: args.plugins_dirs,
        plugin_port: args.plugin_port,
        api_port: args.api_port,
        ui_port: (!args.no_ui).then_some(args.ui_port),
        allow_origins: args.allow_origins,
        virtual_device: args.virtual_device,
        hardware: !args.no_hardware,
        ..Options::new(config_dir)
    };

    let shutdown = CancellationToken::new();
    let mut service = tokio::spawn(n3_daemon::run(opts, shutdown.clone()));

    if !args.no_ui {
        println!("\n  OpenDeckN3 läuft – Oberfläche: {ui_url}\n  Beenden: Strg+C\n");
        if args.open
            && let Err(err) = n3_daemon::open_url(&ui_url)
        {
            tracing::warn!(%err, "cannot open browser");
        }
    }

    tokio::select! {
        result = &mut service => return result?,
        _ = tokio::signal::ctrl_c() => shutdown.cancel(),
    }
    // Wait until plugins are stopped.
    service.await?
}

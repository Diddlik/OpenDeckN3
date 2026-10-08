//! OpenDeckN3 desktop app.
//!
//! A Tauri shell around the web UI (`ui/index.html`) that also runs the
//! OpenDeckN3 service in-process. Closing the window keeps the app running in
//! the tray; "Beenden" in the tray menu stops the service and exits.

// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{fs::File, path::PathBuf, sync::Mutex, time::Duration};

use n3_daemon::Options;
use serde_json::{Value, json};
use tauri::{
    AppHandle, Manager, WindowEvent,
    async_runtime::JoinHandle,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

/// Passed by the autostart entry: start in the tray without showing the window.
const HIDDEN_ARG: &str = "--hidden";
const LOG_FILE: &str = "opendeckn3.log";

struct Service {
    config_dir: PathBuf,
    shutdown: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        window.show().ok();
        window.unminimize().ok();
        window.set_focus().ok();
    }
}

/// Stops the embedded service (plugins included), then exits.
fn quit(app: &AppHandle) {
    let service = app.state::<Service>();
    service.shutdown.cancel();
    let task = service.task.lock().ok().and_then(|mut t| t.take());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(task) = task {
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .ok();
        }
        app.exit(0);
    });
}

fn open_path(app: &AppHandle, path: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().unwrap_or(&path)).ok();
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn desktop_paths(service: tauri::State<'_, Service>) -> Value {
    json!({
        "config": service.config_dir,
        "plugins": service.config_dir.join("plugins"),
        "log": service.config_dir.join(LOG_FILE),
    })
}

#[tauri::command]
fn open_config_dir(app: AppHandle, service: tauri::State<'_, Service>) -> Result<(), String> {
    open_path(&app, service.config_dir.clone())
}

#[tauri::command]
fn open_plugins_dir(app: AppHandle, service: tauri::State<'_, Service>) -> Result<(), String> {
    let dir = service.config_dir.join("plugins");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    open_path(&app, dir)
}

#[tauri::command]
fn open_log(app: AppHandle, service: tauri::State<'_, Service>) -> Result<(), String> {
    open_path(&app, service.config_dir.join(LOG_FILE))
}

/// Without a console, logs go to `<config>/opendeckn3.log` (overwritten per start).
fn init_logging(config_dir: &std::path::Path) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    std::fs::create_dir_all(config_dir).ok();
    match File::create(config_dir.join(LOG_FILE)) {
        Ok(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

fn start_service(app: &tauri::App, config_dir: PathBuf) -> Option<JoinHandle<()>> {
    let mut opts = Options::new(config_dir);
    // The desktop window replaces the browser UI.
    opts.ui_port = None;
    if let Ok(resources) = app.path().resource_dir() {
        let bundled = resources.join("plugins");
        if bundled.is_dir() {
            opts.extra_plugin_dirs.push(bundled);
        }
    }

    if n3_daemon::is_running(opts.api_port) {
        // e.g. `opendeckn3d` started from a terminal – the window just connects to it.
        tracing::warn!("OpenDeckN3 service already running, using the existing one");
        return None;
    }

    let shutdown = app.state::<Service>().shutdown.clone();
    Some(tauri::async_runtime::spawn(async move {
        if let Err(err) = n3_daemon::run(opts, shutdown).await {
            tracing::error!("service failed: {err:#}");
        }
    }))
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "OpenDeckN3 öffnen", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit_item])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("OpenDeckN3")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    let config_dir = n3_daemon::default_config_dir().expect("no config directory");
    init_logging(&config_dir);
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "OpenDeckN3 desktop starting"
    );

    tauri::Builder::default()
        // Must be registered first: a second start only focuses the window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Service {
            config_dir: config_dir.clone(),
            shutdown: CancellationToken::new(),
            task: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            desktop_paths,
            open_config_dir,
            open_plugins_dir,
            open_log
        ])
        .setup(move |app| {
            let task = start_service(app, config_dir.clone());
            if let Ok(mut slot) = app.state::<Service>().task.lock() {
                *slot = task;
            }
            build_tray(app)?;
            if !std::env::args().any(|a| a == HIDDEN_ARG) {
                show_main_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps OpenDeckN3 running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                window.hide().ok();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenDeckN3");
}

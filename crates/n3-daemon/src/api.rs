//! Local WebSocket API for user interfaces (see `docs/UI_API.md`).
//!
//! Requests:  `{"id": 1, "command": "getState"}`
//! Responses: `{"id": 1, "ok": true, "result": ...}` / `{"id": 1, "ok": false, "error": "..."}`
//! Events:    `{"event": "keyImage", ...}` pushed to every client.

use std::{net::SocketAddr, sync::Arc};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use n3_core::{ActionInstance, Controller, InputEvent};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, oneshot},
};
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{ErrorResponse, Request, Response},
    http::StatusCode,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum ApiCommand {
    GetState,
    GetCatalog,
    SwitchProfile {
        device: String,
        profile: String,
    },
    DeleteProfile {
        device: String,
        profile: String,
    },
    CopyProfile {
        device: String,
        profile: String,
        to: String,
    },
    /// Pages of the active profile (index from 0).
    SwitchPage {
        device: String,
        page: usize,
    },
    AddPage {
        device: String,
        #[serde(default)]
        name: Option<String>,
    },
    RenamePage {
        device: String,
        page: usize,
        name: String,
    },
    MovePage {
        device: String,
        page: usize,
        to: usize,
    },
    DeletePage {
        device: String,
        page: usize,
    },
    CopyPage {
        device: String,
        page: usize,
    },
    GetAppSettings,
    /// Partial update, e.g. `{"autoUpdatePlugins": false}`.
    SetAppSettings {
        settings: Value,
    },
    SetAction {
        device: String,
        controller: Controller,
        position: u8,
        plugin: String,
        action: String,
        #[serde(default)]
        settings: Option<Value>,
    },
    /// All profiles of a device as one backup document; with `path` it is
    /// written to that `.json` file instead of returned.
    ExportProfiles {
        device: String,
        #[serde(default)]
        path: Option<String>,
    },
    /// Adds the profiles of an export document; existing profiles are never
    /// overwritten, a clashing name gets a suffix.
    ImportProfiles {
        device: String,
        data: Value,
    },
    /// Puts a complete assignment (settings, title, image) on a slot, or clears
    /// it with `null`. Used for moving, pasting and undo.
    SetSlot {
        device: String,
        controller: Controller,
        position: u8,
        instance: Option<ActionInstance>,
    },
    ClearAction {
        device: String,
        controller: Controller,
        position: u8,
    },
    SetActionSettings {
        device: String,
        controller: Controller,
        position: u8,
        settings: Value,
    },
    SetActionAppearance {
        device: String,
        controller: Controller,
        position: u8,
        #[serde(default)]
        title: Option<String>,
        /// Data URL or `null` to reset to the plugin default.
        #[serde(default)]
        image: Option<String>,
    },
    SetBrightness {
        device: String,
        value: u8,
    },
    SimulateInput {
        device: String,
        input: InputEvent,
    },
    StartVirtualDevice,
    /// Exactly one of `path` (local file), `data` (base64 / data URL) or
    /// `repo` (GitHub `owner/name`, latest release).
    InstallPlugin {
        #[serde(default)]
        path: Option<std::path::PathBuf>,
        #[serde(default)]
        data: Option<String>,
        #[serde(default)]
        repo: Option<String>,
        #[serde(default)]
        asset: Option<String>,
    },
    UninstallPlugin {
        plugin: String,
    },
    GetGlobalSettings {
        plugin: String,
    },
    /// Merges `settings` into the plugin's global settings.
    SetGlobalSettings {
        plugin: String,
        settings: Value,
    },
    /// Catalog from the registries with the latest GitHub release per plugin.
    PluginStore {
        #[serde(default)]
        refresh: bool,
    },
    /// Asks a plugin for data, e.g. options of a settings field (`source` in
    /// a schema). The plugin receives `sendToPlugin` and answers with
    /// `sendToPropertyInspector` carrying the same `requestId`.
    PluginRequest {
        plugin: String,
        payload: Value,
    },
    /// Installs every catalog plugin that has a newer release.
    UpdatePlugins,
    /// Internal: moves an unpacked plugin into place and starts it.
    #[serde(skip_deserializing)]
    ActivatePlugin {
        staged: std::path::PathBuf,
    },
    /// Internal: tells the UIs which plugins were updated in the background.
    #[serde(skip_deserializing)]
    ReportPluginUpdates {
        plugins: Value,
    },
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    id: Value,
    #[serde(flatten)]
    command: ApiCommand,
}

/// A command plus the channel to answer it on.
pub type ApiRequest = (ApiCommand, oneshot::Sender<Result<Value, String>>);

/// Browser origins allowed to connect. Clients without an `Origin` header
/// (native apps, Node scripts) are always allowed; browsers always send one,
/// so arbitrary websites cannot remote-control the deck.
#[derive(Clone, Debug, Default)]
pub struct AllowedOrigins(Vec<String>);

impl AllowedOrigins {
    pub fn new(origins: impl IntoIterator<Item = String>) -> Self {
        Self(
            origins
                .into_iter()
                .map(|o| o.trim_end_matches('/').to_owned())
                .collect(),
        )
    }

    fn permits(&self, origin: Option<&str>) -> bool {
        match origin {
            None => true,
            Some(origin) => self.0.iter().any(|o| o == origin.trim_end_matches('/')),
        }
    }
}

pub async fn serve(
    port: u16,
    origins: AllowedOrigins,
    requests: mpsc::Sender<ApiRequest>,
    events: broadcast::Sender<Value>,
) -> anyhow::Result<()> {
    let origins = Arc::new(origins);
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding UI API port {port}"))?;
    tracing::info!(port, "UI API listening on ws://127.0.0.1:{port}");
    loop {
        let (stream, addr) = listener.accept().await?;
        tokio::spawn(handle_client(
            stream,
            addr,
            origins.clone(),
            requests.clone(),
            events.subscribe(),
        ));
    }
}

async fn handle_client(
    stream: TcpStream,
    addr: SocketAddr,
    origins: Arc<AllowedOrigins>,
    requests: mpsc::Sender<ApiRequest>,
    mut events: broadcast::Receiver<Value>,
) {
    // The callback signature (and its large `Err` type) is fixed by tungstenite.
    #[allow(clippy::result_large_err)]
    let check_origin = |request: &Request, response: Response| {
        let origin = request
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok());
        if origins.permits(origin) {
            Ok(response)
        } else {
            tracing::warn!(%addr, ?origin, "UI connection from foreign origin rejected");
            let mut denied = ErrorResponse::new(Some("origin not allowed".to_owned()));
            *denied.status_mut() = StatusCode::FORBIDDEN;
            Err(denied)
        }
    };
    // Plugin uploads (`installPlugin` with `data`) can be large.
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(300 << 20))
        .max_frame_size(Some(300 << 20));
    let ws =
        match tokio_tungstenite::accept_hdr_async_with_config(stream, check_origin, Some(config))
            .await
        {
            Ok(ws) => ws,
            Err(err) => {
                tracing::debug!(%addr, %err, "UI handshake failed");
                return;
            }
        };
    tracing::info!(%addr, "UI client connected");
    let (mut sink, mut incoming) = ws.split();
    // Replies of slow commands (downloads, plugin queries) must not block the
    // connection: requests are queued in order, replies awaited in parallel.
    let (replies_tx, mut replies) = mpsc::unbounded_channel::<Value>();

    loop {
        let outgoing = tokio::select! {
            msg = incoming.next() => match msg {
                Some(Ok(Message::Text(text))) => match submit(&text, &requests).await {
                    Ok((id, rx)) => {
                        let tx = replies_tx.clone();
                        tokio::spawn(async move {
                            tx.send(reply(id, rx.await)).ok();
                        });
                        continue;
                    }
                    Err(error) => error,
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            },
            Some(reply) = replies.recv() => reply,
            event = events.recv() => match event {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => json!({ "event": "lagged", "missed": n }),
                Err(broadcast::error::RecvError::Closed) => break,
            },
        };
        if sink
            .send(Message::text(outgoing.to_string()))
            .await
            .is_err()
        {
            break;
        }
    }
    tracing::info!(%addr, "UI client disconnected");
}

type Reply = oneshot::Receiver<Result<Value, String>>;

/// Parses a request and queues it for the main loop. `Err` is the error reply.
async fn submit(text: &str, requests: &mpsc::Sender<ApiRequest>) -> Result<(Value, Reply), Value> {
    let envelope: Envelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(err) => {
            let id = serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|v| v.get("id").cloned())
                .unwrap_or(Value::Null);
            return Err(json!({ "id": id, "ok": false, "error": format!("bad request: {err}") }));
        }
    };
    let (tx, rx) = oneshot::channel();
    if requests.send((envelope.command, tx)).await.is_err() {
        return Err(json!({ "id": envelope.id, "ok": false, "error": "daemon is shutting down" }));
    }
    Ok((envelope.id, rx))
}

fn reply(id: Value, result: Result<Result<Value, String>, oneshot::error::RecvError>) -> Value {
    match result {
        Ok(Ok(result)) => json!({ "id": id, "ok": true, "result": result }),
        Ok(Err(error)) => json!({ "id": id, "ok": false, "error": error }),
        Err(_) => {
            json!({ "id": id, "ok": false, "error": "keine Antwort (Plugin nicht erreichbar?)" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        let origins = AllowedOrigins::new(["http://127.0.0.1:57132/".to_owned()]);
        assert!(origins.permits(None));
        assert!(origins.permits(Some("http://127.0.0.1:57132")));
        assert!(!origins.permits(Some("https://evil.example")));
        assert!(!origins.permits(Some("null")));
    }

    #[test]
    fn parses_envelopes() {
        let env: Envelope = serde_json::from_str(
            r#"{"id":7,"command":"setAction","device":"n3-1","controller":"Keypad","position":2,
                "plugin":"opendeckn3.builtin","action":"opendeckn3.builtin.brightness"}"#,
        )
        .unwrap();
        assert_eq!(env.id, json!(7));
        assert!(matches!(
            env.command,
            ApiCommand::SetAction { position: 2, .. }
        ));

        let env: Envelope = serde_json::from_str(
            r#"{"command":"simulateInput","device":"virtual-n3","input":{"type":"encoderTwist","encoder":1,"ticks":-1}}"#,
        )
        .unwrap();
        assert!(matches!(
            env.command,
            ApiCommand::SimulateInput {
                input: InputEvent::EncoderTwist {
                    encoder: 1,
                    ticks: -1
                },
                ..
            }
        ));
    }
}

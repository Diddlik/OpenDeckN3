//! Local WebSocket API for user interfaces (see `docs/UI_API.md`).
//!
//! Requests:  `{"id": 1, "command": "getState"}`
//! Responses: `{"id": 1, "ok": true, "result": ...}` / `{"id": 1, "ok": false, "error": "..."}`
//! Events:    `{"event": "keyImage", ...}` pushed to every client.

use std::net::SocketAddr;

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use n3_core::{Controller, InputEvent};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, oneshot},
};
use tokio_tungstenite::tungstenite::Message;

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
    SetAction {
        device: String,
        controller: Controller,
        position: u8,
        plugin: String,
        action: String,
        #[serde(default)]
        settings: Option<Value>,
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

pub async fn serve(
    port: u16,
    requests: mpsc::Sender<ApiRequest>,
    events: broadcast::Sender<Value>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding UI API port {port}"))?;
    tracing::info!(port, "UI API listening on ws://127.0.0.1:{port}");
    loop {
        let (stream, addr) = listener.accept().await?;
        tokio::spawn(handle_client(
            stream,
            addr,
            requests.clone(),
            events.subscribe(),
        ));
    }
}

async fn handle_client(
    stream: TcpStream,
    addr: SocketAddr,
    requests: mpsc::Sender<ApiRequest>,
    mut events: broadcast::Receiver<Value>,
) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(err) => {
            tracing::debug!(%addr, %err, "UI handshake failed");
            return;
        }
    };
    tracing::info!(%addr, "UI client connected");
    let (mut sink, mut incoming) = ws.split();

    loop {
        let outgoing = tokio::select! {
            msg = incoming.next() => match msg {
                Some(Ok(Message::Text(text))) => handle_request(&text, &requests).await,
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            },
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

async fn handle_request(text: &str, requests: &mpsc::Sender<ApiRequest>) -> Value {
    let envelope: Envelope = match serde_json::from_str(text) {
        Ok(envelope) => envelope,
        Err(err) => {
            let id = serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|v| v.get("id").cloned())
                .unwrap_or(Value::Null);
            return json!({ "id": id, "ok": false, "error": format!("bad request: {err}") });
        }
    };
    let (tx, rx) = oneshot::channel();
    if requests.send((envelope.command, tx)).await.is_err() {
        return json!({ "id": envelope.id, "ok": false, "error": "daemon is shutting down" });
    }
    match rx.await {
        Ok(Ok(result)) => json!({ "id": envelope.id, "ok": true, "result": result }),
        Ok(Err(error)) => json!({ "id": envelope.id, "ok": false, "error": error }),
        Err(_) => json!({ "id": envelope.id, "ok": false, "error": "no response" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

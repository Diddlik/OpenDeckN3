//! Discord's local RPC over IPC (named pipe on Windows, Unix socket elsewhere).
//!
//! Frame: `opcode: u32 LE`, `length: u32 LE`, JSON payload.
//! Requests carry a `nonce`; the reply with the same nonce resolves the call,
//! everything with `cmd: "DISPATCH"` is an event.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// What the reader task reports besides replies.
#[derive(Debug)]
pub enum IpcEvent {
    /// `DISPATCH` event: name and data.
    Dispatch(String, Value),
    Closed(String),
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<anyhow::Result<Value>>>>>;

/// A connected (handshaken) RPC client.
#[derive(Clone)]
pub struct Rpc {
    out: mpsc::UnboundedSender<(u32, Value)>,
    pending: Pending,
    nonce: Arc<AtomicU64>,
}

impl Rpc {
    /// Connects to the first Discord IPC socket that accepts the handshake
    /// for `client_id`. Events go to `events`.
    pub async fn connect(
        client_id: &str,
        events: mpsc::UnboundedSender<IpcEvent>,
    ) -> anyhow::Result<Self> {
        let mut last_err = anyhow!("Discord läuft nicht (keine IPC-Verbindung gefunden)");
        for path in candidate_paths() {
            match open(&path).await {
                Ok(stream) => match Self::handshake(stream, client_id, events.clone()).await {
                    Ok(rpc) => return Ok(rpc),
                    Err(err) => last_err = err,
                },
                Err(_) => continue,
            }
        }
        Err(last_err)
    }

    /// Handshake over an already opened stream (also used by tests).
    pub async fn handshake<S>(
        stream: S,
        client_id: &str,
        events: mpsc::UnboundedSender<IpcEvent>,
    ) -> anyhow::Result<Self>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        write_frame(
            &mut writer,
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": client_id }),
        )
        .await?;
        let (op, ready) = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut reader))
            .await
            .context("Discord antwortet nicht")??;
        match op {
            OP_FRAME if ready["evt"] == "READY" => {}
            OP_CLOSE => bail!(
                "Discord lehnt die Verbindung ab: {} (Client-ID prüfen)",
                ready["message"].as_str().unwrap_or("unbekannter Fehler")
            ),
            _ => bail!("unerwartete Antwort von Discord: {ready}"),
        }

        let (out, mut out_rx) = mpsc::unbounded_channel::<(u32, Value)>();
        tokio::spawn(async move {
            while let Some((op, payload)) = out_rx.recv().await {
                if write_frame(&mut writer, op, &payload).await.is_err() {
                    break;
                }
            }
        });

        let pending: Pending = Default::default();
        let reader_pending = pending.clone();
        let pong = out.clone();
        tokio::spawn(async move {
            let reason = loop {
                let (op, payload) = match read_frame(&mut reader).await {
                    Ok(frame) => frame,
                    Err(err) => break format!("{err:#}"),
                };
                match op {
                    OP_FRAME => {
                        let nonce = payload["nonce"].as_str().map(str::to_owned);
                        if payload["cmd"] == "DISPATCH" || nonce.is_none() {
                            let evt = payload["evt"].as_str().unwrap_or_default().to_owned();
                            events
                                .send(IpcEvent::Dispatch(evt, payload["data"].clone()))
                                .ok();
                        } else if let Some(tx) =
                            nonce.and_then(|n| reader_pending.lock().ok()?.remove(&n))
                        {
                            tx.send(reply_result(&payload)).ok();
                        }
                    }
                    OP_PING => {
                        pong.send((OP_PONG, payload)).ok();
                    }
                    OP_CLOSE => {
                        break payload["message"]
                            .as_str()
                            .unwrap_or("Verbindung beendet")
                            .to_owned();
                    }
                    _ => {}
                }
            };
            // Fail all outstanding calls.
            if let Ok(mut pending) = reader_pending.lock() {
                for (_, tx) in pending.drain() {
                    tx.send(Err(anyhow!("Verbindung zu Discord getrennt"))).ok();
                }
            }
            events.send(IpcEvent::Closed(reason)).ok();
        });

        Ok(Self {
            out,
            pending,
            nonce: Default::default(),
        })
    }

    /// Sends a command and waits for its reply (`timeout` for popups etc.).
    pub async fn call_with_timeout(
        &self,
        cmd: &str,
        args: Value,
        evt: Option<&str>,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let nonce = format!("n{}", self.nonce.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("poisoned"))?
            .insert(nonce.clone(), tx);
        let mut frame = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        if let Some(evt) = evt {
            frame["evt"] = json!(evt);
        }
        self.out
            .send((OP_FRAME, frame))
            .map_err(|_| anyhow!("Verbindung zu Discord getrennt"))?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!("Verbindung zu Discord getrennt")),
            Err(_) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&nonce);
                }
                Err(anyhow!("Discord antwortet nicht ({cmd})"))
            }
        }
    }

    pub async fn call(&self, cmd: &str, args: Value) -> anyhow::Result<Value> {
        self.call_with_timeout(cmd, args, None, Duration::from_secs(10))
            .await
    }

    pub async fn subscribe(&self, evt: &str, args: Value) -> anyhow::Result<Value> {
        self.call_with_timeout("SUBSCRIBE", args, Some(evt), Duration::from_secs(10))
            .await
    }

    pub fn close(&self) {
        self.out.send((OP_CLOSE, json!({}))).ok();
    }
}

fn reply_result(payload: &Value) -> anyhow::Result<Value> {
    if payload["evt"] == "ERROR" {
        let code = payload["data"]["code"].as_i64().unwrap_or_default();
        let message = payload["data"]["message"].as_str().unwrap_or("Fehler");
        Err(anyhow!("{message} (Code {code})"))
    } else {
        Ok(payload["data"].clone())
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    op: u32,
    payload: &Value,
) -> anyhow::Result<()> {
    let body = serde_json::to_vec(payload)?;
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> anyhow::Result<(u32, Value)> {
    let mut header = [0u8; 8];
    r.read_exact(&mut header).await?;
    let op = u32::from_le_bytes(header[..4].try_into()?);
    let len = u32::from_le_bytes(header[4..].try_into()?);
    anyhow::ensure!(len <= MAX_FRAME, "IPC frame too large");
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok((op, serde_json::from_slice(&body)?))
}

#[cfg(windows)]
fn candidate_paths() -> Vec<String> {
    (0..10)
        .map(|i| format!(r"\\.\pipe\discord-ipc-{i}"))
        .collect()
}

#[cfg(not(windows))]
fn candidate_paths() -> Vec<String> {
    let mut bases: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .collect();
    bases.push("/tmp".into());
    // Flatpak / Snap / Vesktop put the socket into sub-directories.
    let subdirs = [
        "",
        "app/com.discordapp.Discord/",
        "app/com.discordapp.DiscordCanary/",
        "snap.discord/",
        ".flatpak/dev.vencord.Vesktop/xdg-run/",
    ];
    let mut paths = Vec::new();
    for base in &bases {
        for sub in subdirs {
            for i in 0..10 {
                paths.push(format!(
                    "{}/{sub}discord-ipc-{i}",
                    base.trim_end_matches('/')
                ));
            }
        }
    }
    paths
}

#[cfg(windows)]
async fn open(path: &str) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    tokio::net::windows::named_pipe::ClientOptions::new().open(path)
}

#[cfg(not(windows))]
async fn open(path: &str) -> std::io::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(path).await
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Minimal fake Discord on the other end of a duplex stream: answers the
    /// handshake, then hands every command frame to `respond`.
    pub fn fake_discord<F>(respond: F) -> tokio::io::DuplexStream
    where
        F: Fn(&Value) -> Vec<Value> + Send + 'static,
    {
        let (client, server) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(server);
            let (op, hello) = read_frame(&mut r).await.unwrap();
            assert_eq!(op, OP_HANDSHAKE);
            assert_eq!(hello["v"], 1);
            write_frame(
                &mut w,
                OP_FRAME,
                &json!({ "cmd": "DISPATCH", "evt": "READY", "data": {} }),
            )
            .await
            .unwrap();
            while let Ok((op, frame)) = read_frame(&mut r).await {
                if op != OP_FRAME {
                    continue;
                }
                for answer in respond(&frame) {
                    write_frame(&mut w, OP_FRAME, &answer).await.unwrap();
                }
            }
        });
        client
    }

    #[tokio::test]
    async fn call_reply_and_dispatch() {
        let stream = fake_discord(|f| {
            let reply =
                json!({ "cmd": f["cmd"], "nonce": f["nonce"], "data": { "echo": f["args"] } });
            let event = json!({ "cmd": "DISPATCH", "evt": "VOICE_SETTINGS_UPDATE", "data": { "mute": true } });
            vec![event, reply]
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let rpc = Rpc::handshake(stream, "123", tx).await.unwrap();
        let data = rpc
            .call("GET_VOICE_SETTINGS", json!({ "x": 1 }))
            .await
            .unwrap();
        assert_eq!(data["echo"]["x"], 1);
        match rx.recv().await.unwrap() {
            IpcEvent::Dispatch(evt, data) => {
                assert_eq!(evt, "VOICE_SETTINGS_UPDATE");
                assert_eq!(data["mute"], true);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn error_replies_become_errors() {
        let stream = fake_discord(|f| {
            vec![
                json!({ "cmd": f["cmd"], "nonce": f["nonce"], "evt": "ERROR",
                "data": { "code": 4006, "message": "Not authenticated" } }),
            ]
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let rpc = Rpc::handshake(stream, "123", tx).await.unwrap();
        let err = rpc.call("SET_VOICE_SETTINGS", json!({})).await.unwrap_err();
        assert!(err.to_string().contains("4006"), "{err}");
    }
}

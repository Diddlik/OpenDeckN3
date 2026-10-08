//! obs-websocket v5 client (built into OBS Studio 28+).
//!
//! Messages are JSON `{op, d}`: 0 Hello, 1 Identify, 2 Identified,
//! 5 Event, 6 Request, 7 RequestResponse.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};

/// General, Config, Scenes, Inputs, Transitions, Filters, Outputs,
/// SceneItems, MediaInputs, Vendors, Ui – everything except high-volume events.
const EVENT_SUBSCRIPTIONS: u32 = 0x7FF;

#[derive(Debug)]
pub enum ObsEvent {
    Event(String, Value),
    Closed(String),
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<anyhow::Result<Value>>>>>;

#[derive(Clone)]
pub struct Obs {
    out: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next: Arc<AtomicU64>,
}

/// Connection data: host, port, password.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub password: String,
}

/// Reads host/port/password of the local OBS from its obs-websocket
/// configuration (`obs-studio/plugin_config/obs-websocket/config.json`).
pub fn local_endpoint() -> anyhow::Result<Endpoint> {
    let path = local_config_path().context("Konfigurationsordner nicht gefunden")?;
    let text = std::fs::read_to_string(&path).map_err(|_| {
        anyhow!("OBS-Einstellungen nicht gefunden – ist OBS Studio (ab Version 28) installiert?")
    })?;
    let config: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    if config["server_enabled"] == false {
        bail!(
            "In OBS ist der WebSocket-Server aus: Werkzeuge → WebSocket-Servereinstellungen → „WebSocket-Server aktivieren“"
        );
    }
    let password = if config["auth_required"] == false {
        String::new()
    } else {
        config["server_password"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    Ok(Endpoint {
        host: "127.0.0.1".into(),
        port: config["server_port"].as_u64().unwrap_or(4455) as u16,
        password,
    })
}

fn local_config_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("obs-studio")
            .join("plugin_config")
            .join("obs-websocket")
            .join("config.json"),
    )
}

/// `base64(sha256(base64(sha256(password + salt)) + challenge))`
pub fn auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = b64.encode(Sha256::digest(format!("{password}{salt}").as_bytes()));
    b64.encode(Sha256::digest(format!("{secret}{challenge}").as_bytes()))
}

impl Obs {
    pub async fn connect(
        endpoint: &Endpoint,
        events: mpsc::UnboundedSender<ObsEvent>,
    ) -> anyhow::Result<Self> {
        let url = format!("ws://{}:{}", endpoint.host, endpoint.port);
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(&url),
        )
        .await
        .map_err(|_| anyhow!("OBS antwortet nicht ({url})"))?
        .map_err(|_| anyhow!("OBS nicht erreichbar ({url}) – läuft OBS?"))?;
        let (mut sink, mut stream) = ws.split();

        let hello = next_json(&mut stream).await?;
        anyhow::ensure!(hello["op"] == 0, "unerwartete Antwort von OBS");
        let mut identify = json!({ "rpcVersion": 1, "eventSubscriptions": EVENT_SUBSCRIPTIONS });
        if let Some(auth) = hello["d"].get("authentication") {
            if endpoint.password.is_empty() {
                bail!("OBS verlangt ein Passwort");
            }
            identify["authentication"] = json!(auth_string(
                &endpoint.password,
                auth["salt"].as_str().unwrap_or_default(),
                auth["challenge"].as_str().unwrap_or_default(),
            ));
        }
        sink.send(Message::text(json!({ "op": 1, "d": identify }).to_string()))
            .await?;
        let identified = next_json(&mut stream).await?;
        anyhow::ensure!(
            identified["op"] == 2,
            "OBS hat die Anmeldung nicht bestätigt"
        );

        let (out, mut out_rx) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });
        let pending: Pending = Default::default();
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let reason = loop {
                let msg = match stream.next().await {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(frame))) => {
                        break frame.map(|f| f.reason.to_string()).unwrap_or_default();
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => break err.to_string(),
                    None => break "Verbindung beendet".into(),
                };
                let Ok(msg) = serde_json::from_str::<Value>(&msg) else {
                    continue;
                };
                let d = &msg["d"];
                match msg["op"].as_u64() {
                    Some(5) => {
                        let kind = d["eventType"].as_str().unwrap_or_default().to_owned();
                        events
                            .send(ObsEvent::Event(kind, d["eventData"].clone()))
                            .ok();
                    }
                    Some(7) => {
                        let id = d["requestId"].as_str().unwrap_or_default();
                        if let Some(tx) = reader_pending.lock().ok().and_then(|mut p| p.remove(id))
                        {
                            let status = &d["requestStatus"];
                            let result = if status["result"] == true {
                                Ok(d["responseData"].clone())
                            } else {
                                Err(anyhow!(
                                    "{} (Code {})",
                                    status["comment"].as_str().unwrap_or("OBS lehnt ab"),
                                    status["code"]
                                ))
                            };
                            tx.send(result).ok();
                        }
                    }
                    _ => {}
                }
            };
            if let Ok(mut pending) = reader_pending.lock() {
                for (_, tx) in pending.drain() {
                    tx.send(Err(anyhow!("Verbindung zu OBS getrennt"))).ok();
                }
            }
            events.send(ObsEvent::Closed(reason)).ok();
        });
        Ok(Self {
            out,
            pending,
            next: Default::default(),
        })
    }

    pub async fn call(&self, request: &str, data: Value) -> anyhow::Result<Value> {
        let id = format!("r{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("poisoned"))?
            .insert(id.clone(), tx);
        let mut d = json!({ "requestType": request, "requestId": id });
        if !data.is_null() {
            d["requestData"] = data;
        }
        self.out
            .send(Message::text(json!({ "op": 6, "d": d }).to_string()))
            .map_err(|_| anyhow!("Verbindung zu OBS getrennt"))?;
        match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(anyhow!("Verbindung zu OBS getrennt")),
            Err(_) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(anyhow!("OBS antwortet nicht ({request})"))
            }
        }
    }

    pub fn close(&self) {
        self.out.send(Message::Close(None)).ok();
    }
}

async fn next_json<S>(stream: &mut S) -> anyhow::Result<Value>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .map_err(|_| anyhow!("OBS antwortet nicht"))?;
        match msg {
            Some(Ok(Message::Text(text))) => return Ok(serde_json::from_str(&text)?),
            Some(Ok(Message::Close(Some(frame)))) if frame.code == CloseCode::Library(4009) => {
                bail!("OBS: Passwort falsch")
            }
            Some(Ok(Message::Close(frame))) => bail!(
                "OBS hat die Verbindung beendet{}",
                frame.map(|f| format!(": {}", f.reason)).unwrap_or_default()
            ),
            Some(Ok(_)) => continue,
            Some(Err(err)) => return Err(err.into()),
            None => bail!("Verbindung zu OBS beendet"),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Fake OBS: answers the handshake (with password `pw`) and every request
    /// via `respond(requestType, requestData) -> responseData`, and sends
    /// the events `respond` returns for `"__events__"` right after identifying.
    pub async fn fake_obs<F>(respond: F) -> (u16, Arc<Mutex<Vec<Value>>>)
    where
        F: Fn(&str, &Value) -> Value + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let respond = Arc::new(respond);
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let (log, respond) = (log2.clone(), respond.clone());
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                    let hello = json!({ "op": 0, "d": { "rpcVersion": 1,
                        "authentication": { "challenge": "ch", "salt": "sa" } } });
                    ws.send(Message::text(hello.to_string())).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = ws.next().await {
                        let msg: Value = serde_json::from_str(&text).unwrap();
                        log.lock().unwrap().push(msg.clone());
                        let d = &msg["d"];
                        match msg["op"].as_u64() {
                            Some(1) => {
                                if d["authentication"] != auth_string("pw", "sa", "ch") {
                                    ws.close(Some(
                                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                            code: CloseCode::Library(4009),
                                            reason: "Authentication failed.".into(),
                                        },
                                    ))
                                    .await
                                    .ok();
                                    return;
                                }
                                ws.send(Message::text(
                                    json!({ "op": 2, "d": { "negotiatedRpcVersion": 1 } })
                                        .to_string(),
                                ))
                                .await
                                .unwrap();
                                if let Value::Array(events) = respond("__events__", &Value::Null) {
                                    for e in events {
                                        ws.send(Message::text(
                                            json!({ "op": 5, "d": e }).to_string(),
                                        ))
                                        .await
                                        .unwrap();
                                    }
                                }
                            }
                            Some(6) => {
                                let kind = d["requestType"].as_str().unwrap();
                                let data = respond(kind, &d["requestData"]);
                                let reply = json!({ "op": 7, "d": { "requestType": kind, "requestId": d["requestId"],
                                    "requestStatus": { "result": true, "code": 100 }, "responseData": data } });
                                ws.send(Message::text(reply.to_string())).await.unwrap();
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        (port, log)
    }

    fn endpoint(port: u16, password: &str) -> Endpoint {
        Endpoint {
            host: "127.0.0.1".into(),
            port,
            password: password.into(),
        }
    }

    #[tokio::test]
    async fn handshake_auth_requests_and_events() {
        let (port, log) = fake_obs(|kind, _| match kind {
            "GetVersion" => json!({ "obsVersion": "31.0.0" }),
            "__events__" => json!([{ "eventType": "CurrentProgramSceneChanged", "eventData": { "sceneName": "Szene 2" } }]),
            _ => json!({}),
        })
        .await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let obs = Obs::connect(&endpoint(port, "pw"), tx).await.unwrap();
        let v = obs.call("GetVersion", Value::Null).await.unwrap();
        assert_eq!(v["obsVersion"], "31.0.0");
        match rx.recv().await.unwrap() {
            ObsEvent::Event(kind, data) => {
                assert_eq!(kind, "CurrentProgramSceneChanged");
                assert_eq!(data["sceneName"], "Szene 2");
            }
            other => panic!("{other:?}"),
        }
        let identify = log.lock().unwrap()[0].clone();
        assert_eq!(identify["d"]["eventSubscriptions"], EVENT_SUBSCRIPTIONS);
    }

    #[tokio::test]
    async fn wrong_password_is_reported() {
        let (port, _) = fake_obs(|_, _| json!({})).await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let err = Obs::connect(&endpoint(port, "falsch"), tx)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("Passwort"), "{err}");
        let (tx, _rx) = mpsc::unbounded_channel();
        let err = Obs::connect(&endpoint(port, ""), tx).await.err().unwrap();
        assert!(err.to_string().contains("Passwort"), "{err}");
    }

    #[test]
    fn auth_matches_protocol_example() {
        // Example from the obs-websocket protocol description (docs/generated/protocol.md).
        assert_eq!(
            auth_string(
                "supersecretpassword",
                "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="
            ),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }
}

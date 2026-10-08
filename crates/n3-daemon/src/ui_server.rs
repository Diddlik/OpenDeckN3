//! Serves the bundled web UI (`ui/index.html`) on a local HTTP port.
//!
//! The UI is compiled into the binary, so `opendeckn3d` is self-contained.
//! It only needs to answer a handful of GET requests, hence no HTTP framework.

use anyhow::Context;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const INDEX_HTML: &str = include_str!("../../../ui/index.html");

pub async fn serve(port: u16, api_port: u16) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("binding UI port {port}"))?;
    let index = INDEX_HTML.replace("{{API_PORT}}", &api_port.to_string());
    tracing::info!(port, "UI available at http://127.0.0.1:{port}/");
    loop {
        let (stream, _) = listener.accept().await?;
        let index = index.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(stream, &index).await {
                tracing::debug!(%err, "UI request failed");
            }
        });
    }
}

async fn handle(mut stream: TcpStream, index: &str) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut parts = request.lines().next().unwrap_or_default().split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

    let (status, content_type, body) = match (method, path.split('?').next().unwrap_or("")) {
        ("GET" | "HEAD", "/" | "/index.html") => ("200 OK", "text/html; charset=utf-8", index),
        ("GET" | "HEAD", _) => ("404 Not Found", "text/plain; charset=utf-8", "not found"),
        _ => (
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            "method not allowed",
        ),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         X-Frame-Options: DENY\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    if method != "HEAD" {
        stream.write_all(body.as_bytes()).await?;
    }
    stream.shutdown().await.ok();
    Ok(())
}

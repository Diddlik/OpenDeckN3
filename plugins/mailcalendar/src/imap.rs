//! Just enough IMAP (RFC 9051/3501) to count unread mails: LOGIN, STATUS,
//! LOGOUT over implicit TLS.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, crypto::ring, pki_types::ServerName},
};

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

impl Account {
    /// `server` is `host` or `host:port` (default 993, implicit TLS).
    pub fn new(server: &str, user: &str, password: &str) -> anyhow::Result<Self> {
        let server = server.trim();
        anyhow::ensure!(
            !server.is_empty(),
            "IMAP-Server eintragen, z. B. imap.gmail.com"
        );
        let (host, port) = match server.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse()
                    .map_err(|_| anyhow!("ungültiger Port in „{server}“"))?,
            ),
            None => (server, 993),
        };
        anyhow::ensure!(
            !user.trim().is_empty(),
            "Benutzername (E-Mail-Adresse) fehlt"
        );
        anyhow::ensure!(!password.is_empty(), "App-Passwort fehlt");
        Ok(Self {
            host: host.to_owned(),
            port,
            user: user.trim().to_owned(),
            password: password.to_owned(),
        })
    }

    /// Unread mails per folder.
    pub async fn unseen(&self, folders: &[String]) -> anyhow::Result<HashMap<String, u32>> {
        tokio::time::timeout(TIMEOUT, async {
            let stream = self.connect().await?;
            session(stream, &self.user, &self.password, folders).await
        })
        .await
        .map_err(|_| anyhow!("{} antwortet nicht", self.host))?
    }

    async fn connect(
        &self,
    ) -> anyhow::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("{} nicht erreichbar", self.host))?;
        let name = ServerName::try_from(self.host.clone())?;
        TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .with_context(|| format!("TLS-Verbindung zu {} fehlgeschlagen", self.host))
    }
}

/// IMAP quoted string; CR/LF cannot be quoted.
fn quote(text: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !text.contains(['\r', '\n']),
        "Zeilenumbrüche sind nicht erlaubt"
    );
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

struct Conn<S> {
    io: BufReader<S>,
    tag: u32,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Conn<S> {
    async fn line(&mut self) -> anyhow::Result<String> {
        let mut line = String::new();
        anyhow::ensure!(
            self.io.read_line(&mut line).await? > 0,
            "Verbindung vom Server getrennt"
        );
        Ok(line.trim_end().to_owned())
    }

    /// Sends a command; returns the untagged lines and the tagged status
    /// (`OK …`, `NO …`, `BAD …`).
    async fn command(&mut self, command: &str) -> anyhow::Result<(Vec<String>, String)> {
        self.tag += 1;
        let tag = format!("a{}", self.tag);
        self.io
            .get_mut()
            .write_all(format!("{tag} {command}\r\n").as_bytes())
            .await?;
        self.io.get_mut().flush().await?;
        let mut untagged = Vec::new();
        loop {
            let line = self.line().await?;
            if let Some(status) = line.strip_prefix(&format!("{tag} ")) {
                return Ok((untagged, status.to_owned()));
            }
            untagged.push(line);
        }
    }
}

fn is_ok(status: &str) -> bool {
    status
        .get(..2)
        .is_some_and(|s| s.eq_ignore_ascii_case("OK"))
}

/// `* STATUS INBOX (UNSEEN 3)` → 3. A literal mailbox name puts the
/// attributes on the following line, so every line is searched.
fn unseen_count(lines: &[String]) -> Option<u32> {
    lines.iter().find_map(|line| {
        let upper = line.to_ascii_uppercase();
        let rest = &line[upper.find("UNSEEN ")? + 7..];
        rest.split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    })
}

async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    user: &str,
    password: &str,
    folders: &[String],
) -> anyhow::Result<HashMap<String, u32>> {
    let mut conn = Conn {
        io: BufReader::new(stream),
        tag: 0,
    };
    let greeting = conn.line().await?;
    anyhow::ensure!(
        greeting.starts_with("* OK"),
        "unerwartete Begrüßung: {greeting}"
    );
    let (_, status) = conn
        .command(&format!("LOGIN {} {}", quote(user)?, quote(password)?))
        .await?;
    if !is_ok(&status) {
        bail!("Anmeldung abgelehnt – Benutzername und App-Passwort prüfen ({status})");
    }
    let mut counts = HashMap::new();
    for folder in folders {
        let (lines, status) = conn
            .command(&format!("STATUS {} (UNSEEN)", quote(folder)?))
            .await?;
        if !is_ok(&status) {
            bail!("Ordner „{folder}“ nicht gefunden ({status})");
        }
        let count =
            unseen_count(&lines).with_context(|| format!("keine Antwort für „{folder}“"))?;
        counts.insert(folder.clone(), count);
    }
    conn.command("LOGOUT").await.ok();
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays the server side: answers each tagged command with `reply(command)`.
    async fn fake_server(
        stream: tokio::io::DuplexStream,
        reply: fn(&str) -> Vec<&'static str>,
    ) -> Vec<String> {
        let mut io = BufReader::new(stream);
        io.get_mut()
            .write_all(b"* OK IMAP4rev1 ready\r\n")
            .await
            .unwrap();
        let mut seen = Vec::new();
        loop {
            let mut line = String::new();
            if io.read_line(&mut line).await.unwrap() == 0 {
                return seen;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            seen.push(command.to_owned());
            for out in reply(command) {
                let out = out.replace("{tag}", tag);
                io.get_mut()
                    .write_all(format!("{out}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            if command == "LOGOUT" {
                return seen;
            }
        }
    }

    #[tokio::test]
    async fn counts_unseen_per_folder() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(fake_server(server, |cmd| {
            if cmd.starts_with("STATUS \"INBOX\"") {
                vec!["* STATUS INBOX (UNSEEN 3)", "{tag} OK done"]
            } else if cmd.starts_with("STATUS") {
                vec!["* STATUS {9}", "Rechnung\" (UNSEEN 12)", "{tag} OK done"]
            } else if cmd == "LOGOUT" {
                vec!["* BYE", "{tag} OK bye"]
            } else {
                vec!["{tag} OK logged in"]
            }
        }));
        let folders = ["INBOX".to_owned(), "Rechnung\"".to_owned()];
        let counts = session(client, "me@example.com", "ab\"c\\d", &folders)
            .await
            .unwrap();
        assert_eq!(counts["INBOX"], 3);
        assert_eq!(counts["Rechnung\""], 12);
        let seen = server.await.unwrap();
        assert_eq!(seen[0], r#"LOGIN "me@example.com" "ab\"c\\d""#);
        assert_eq!(seen[2], r#"STATUS "Rechnung\"" (UNSEEN)"#);
    }

    #[tokio::test]
    async fn rejected_login_is_reported() {
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(fake_server(server, |_| {
            vec!["{tag} NO [AUTHENTICATIONFAILED] Invalid credentials"]
        }));
        let err = session(client, "me", "wrong", &["INBOX".to_owned()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Anmeldung abgelehnt"), "{err}");
    }

    #[test]
    fn account_and_quoting() {
        let a = Account::new(" mail.example.com:143 ", "u", "p").unwrap();
        assert_eq!((a.host.as_str(), a.port), ("mail.example.com", 143));
        assert_eq!(Account::new("imap.gmail.com", "u", "p").unwrap().port, 993);
        assert!(Account::new("imap.gmail.com", "u", "").is_err());
        assert!(quote("a\r\nb").is_err());
    }
}

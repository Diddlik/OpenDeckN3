//! MPRIS over D-Bus: the player is the first `org.mpris.MediaPlayer2.*` service
//! that is playing, otherwise the first one found.

use std::collections::HashMap;

use anyhow::{Context, Result};
use zbus::{Connection, fdo::DBusProxy, zvariant::OwnedValue};

use crate::{Command, Snapshot};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";

#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait Player {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    /// Relative offset in microseconds.
    fn seek(&self, offset: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

pub async fn snapshot() -> Result<Option<Snapshot>> {
    let conn = Connection::session().await?;
    let Some((player, playing)) = current_player(&conn).await? else {
        return Ok(None);
    };
    let metadata = player.metadata().await.unwrap_or_default();
    Ok(Some(Snapshot {
        title: string_entry(&metadata, "xesam:title").unwrap_or_default(),
        playing,
        cover: string_entry(&metadata, "mpris:artUrl").and_then(|url| cover_bytes(&url)),
    }))
}

pub async fn command(cmd: Command) -> Result<()> {
    let conn = Connection::session().await?;
    let (player, _) = current_player(&conn)
        .await?
        .context("kein Medienplayer gefunden")?;
    match cmd {
        Command::PlayPause => player.play_pause().await?,
        Command::Next => player.next().await?,
        Command::Previous => player.previous().await?,
        Command::Seek(secs) => player.seek((secs * 1_000_000.0) as i64).await?,
    }
    Ok(())
}

async fn current_player(conn: &Connection) -> Result<Option<(PlayerProxy<'_>, bool)>> {
    let mut players = Vec::new();
    for name in DBusProxy::new(conn).await?.list_names().await? {
        if !name.starts_with(MPRIS_PREFIX) {
            continue;
        }
        let player = PlayerProxy::builder(conn)
            .destination(name)?
            .build()
            .await?;
        // A player that does not answer its status counts as paused.
        let playing = player.playback_status().await.is_ok_and(|s| s == "Playing");
        players.push((player, playing));
    }
    Ok(pick_player(players))
}

/// The first playing player, otherwise the first one.
fn pick_player<T>(players: Vec<(T, bool)>) -> Option<(T, bool)> {
    let mut first = None;
    for (player, playing) in players {
        if playing {
            return Some((player, true));
        }
        first.get_or_insert((player, false));
    }
    first
}

fn string_entry(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    String::try_from(metadata.get(key)?.clone()).ok()
}

/// Only local `file://` covers; web covers (e.g. Spotify) are not downloaded.
fn cover_bytes(art_url: &str) -> Option<Vec<u8>> {
    let path = url::Url::parse(art_url).ok()?.to_file_path().ok()?;
    std::fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_player_wins() {
        assert_eq!(
            pick_player(vec![("a", false), ("b", true), ("c", true)]),
            Some(("b", true))
        );
        assert_eq!(
            pick_player(vec![("a", false), ("b", false)]),
            Some(("a", false))
        );
        assert_eq!(pick_player::<()>(vec![]), None);
    }
}

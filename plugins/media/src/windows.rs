//! System media transport controls (SMTC) of the current media session.

use anyhow::{Context, Result};
use windows::{
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    },
    Storage::Streams::{DataReader, IRandomAccessStreamReference},
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
};

use crate::{Command, Snapshot};

/// SMTC calls are blocking here, so they run on the blocking pool.
pub async fn snapshot() -> Result<Option<Snapshot>> {
    tokio::task::spawn_blocking(|| with_winrt(read_snapshot)).await?
}

pub async fn command(cmd: Command) -> Result<()> {
    tokio::task::spawn_blocking(move || with_winrt(|| run_command(cmd))).await?
}

/// Blocking pool threads start without WinRT. Repeated initialization returns
/// S_FALSE or RPC_E_CHANGED_MODE, which is fine to ignore.
fn with_winrt<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    f()
}

fn current_session() -> Result<Option<Session>> {
    Ok(Manager::RequestAsync()?.get()?.GetCurrentSession().ok())
}

fn read_snapshot() -> Result<Option<Snapshot>> {
    let Some(session) = current_session()? else {
        return Ok(None);
    };
    let props = session.TryGetMediaPropertiesAsync()?.get()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == Status::Playing;
    let cover = props
        .Thumbnail()
        .ok()
        .and_then(|reference| read_thumbnail(&reference).ok());
    Ok(Some(Snapshot {
        title: props.Title()?.to_string(),
        playing,
        cover,
    }))
}

fn read_thumbnail(reference: &IRandomAccessStreamReference) -> Result<Vec<u8>> {
    let stream = reference.OpenReadAsync()?.get()?;
    let size = u32::try_from(stream.Size()?)?;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.get()?;
    let mut bytes = vec![0; size as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

fn run_command(cmd: Command) -> Result<()> {
    let session = current_session()?.context("keine Wiedergabe gefunden")?;
    match cmd {
        Command::PlayPause => {
            session.TryTogglePlayPauseAsync()?.get()?;
        }
        Command::Next => {
            session.TrySkipNextAsync()?.get()?;
        }
        Command::Previous => {
            session.TrySkipPreviousAsync()?.get()?;
        }
        Command::Seek(secs) => {
            // SMTC only takes an absolute position, in 100 ns ticks.
            let position = session.GetTimelineProperties()?.Position()?.Duration;
            let target = (position + (secs * 10_000_000.0) as i64).max(0);
            session.TryChangePlaybackPositionAsync(target)?.get()?;
        }
    }
    Ok(())
}

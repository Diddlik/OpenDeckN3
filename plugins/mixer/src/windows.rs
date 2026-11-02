//! WASAPI audio sessions of the default output device.
//!
//! COM is used from one dedicated thread only. Jobs are queued on a channel
//! and answered through a oneshot channel, so tokio workers never touch COM.

use std::{path::Path, sync::OnceLock};

use anyhow::{Context, Result, anyhow};
use windows::{
    Win32::{
        Foundation::CloseHandle,
        Media::Audio::{
            IAudioSessionControl2, IAudioSessionEnumerator, IAudioSessionManager2,
            IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eMultimedia, eRender,
        },
        System::{
            Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
    },
    core::{Interface, PWSTR},
};

use crate::{Change, Session, Snapshot, same_app, step_volume};

type Job = Box<dyn FnOnce() + Send>;

pub async fn snapshot() -> Result<Snapshot> {
    on_com(|| {
        let sessions = audio_sessions()?
            .into_iter()
            .filter_map(|(app, volume)| unsafe {
                Some(Session {
                    app,
                    volume: volume.GetMasterVolume().ok()?,
                    muted: volume.GetMute().ok()?.as_bool(),
                })
            })
            .collect();
        Ok(Snapshot {
            sessions,
            foreground: foreground_app(),
        })
    })
    .await
}

pub async fn change(app: String, change: Change) -> Result<()> {
    on_com(move || {
        let volumes: Vec<ISimpleAudioVolume> = audio_sessions()?
            .into_iter()
            .filter(|(name, _)| same_app(name, &app))
            .map(|(_, volume)| volume)
            .collect();
        match change {
            Change::VolumeStep(delta) => {
                for volume in &volumes {
                    unsafe {
                        let level = step_volume(volume.GetMasterVolume()?, delta);
                        volume.SetMasterVolume(level, std::ptr::null())?;
                    }
                }
            }
            Change::ToggleMute => {
                let Some(first) = volumes.first() else {
                    return Ok(());
                };
                let muted = !unsafe { first.GetMute()? }.as_bool();
                for volume in &volumes {
                    unsafe { volume.SetMute(muted, std::ptr::null())? };
                }
            }
        }
        Ok(())
    })
    .await
}

/// Runs `job` on the COM thread and waits for its result.
async fn on_com<T: Send + 'static>(job: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    com_thread()
        .send(Box::new(move || {
            tx.send(job()).ok();
        }))
        .map_err(|_| anyhow!("Audio-Thread beendet"))?;
    rx.await.context("Audio-Thread antwortet nicht")?
}

fn com_thread() -> &'static std::sync::mpsc::Sender<Job> {
    static WORKER: OnceLock<std::sync::mpsc::Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::spawn(move || {
            // A failure here shows up as an error from the first COM call.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            for job in rx {
                job();
            }
        });
        tx
    })
}

/// Sessions of the default output device, skipping system sounds (process 0).
fn audio_sessions() -> Result<Vec<(String, ISimpleAudioVolume)>> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)? };
    let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None)? };
    let list: IAudioSessionEnumerator = unsafe { manager.GetSessionEnumerator()? };
    let count = unsafe { list.GetCount()? };
    Ok((0..count)
        .filter_map(|index| audio_session(&list, index))
        .collect())
}

fn audio_session(
    list: &IAudioSessionEnumerator,
    index: i32,
) -> Option<(String, ISimpleAudioVolume)> {
    let control = unsafe { list.GetSession(index) }.ok()?;
    let pid = unsafe { control.cast::<IAudioSessionControl2>().ok()?.GetProcessId() }.ok()?;
    if pid == 0 {
        return None;
    }
    let app = process_name(pid)?;
    let volume = control.cast::<ISimpleAudioVolume>().ok()?;
    Some((app, volume))
}

/// File name of the process executable, e.g. `Discord.exe`.
fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = [0u16; 1024];
        let mut len = path.len() as u32;
        let queried = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        queried.ok()?;
        let path = String::from_utf16_lossy(&path[..len as usize]);
        Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

fn foreground_app() -> Option<String> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    process_name(pid)
}

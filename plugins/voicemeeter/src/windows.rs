//! VoiceMeeter Remote API (`VoicemeeterRemote64.dll`), loaded at runtime.
//!
//! The DLL is used from one dedicated thread only. Jobs arrive over a channel
//! and are answered through a oneshot channel. Between jobs the thread polls
//! `VBVMR_IsParametersDirty` and sends fresh values to the main loop.

use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_long, c_void},
    path::{Path, PathBuf},
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use libloading::Library;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
    },
    core::w,
};

use crate::{Command, Event, Layout, Snapshot};

const OFFLINE: &str = "VoiceMeeter läuft nicht";
const NOT_INSTALLED: &str = "VoiceMeeter ist nicht installiert";
const DEFAULT_DIR: &str = r"C:\Program Files (x86)\VB\Voicemeeter";
const POLL: Duration = Duration::from_millis(100);
const RETRY: Duration = Duration::from_secs(5);

type Job = Box<dyn FnOnce(&mut Link) + Send>;

// Signatures from VoicemeeterRemote.h. The header declares __stdcall, which is
// the only calling convention on x64.
type Status = unsafe extern "system" fn() -> c_long;
type GetType = unsafe extern "system" fn(*mut c_long) -> c_long;
type GetFloat = unsafe extern "system" fn(*const c_char, *mut f32) -> c_long;
type SetFloat = unsafe extern "system" fn(*const c_char, f32) -> c_long;
type SetScript = unsafe extern "system" fn(*const c_char) -> c_long;

static JOBS: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

/// Starts the DLL thread, which reports to `events`. Call once at startup.
pub fn start(events: UnboundedSender<Event>) {
    let (tx, rx) = mpsc::channel::<Job>();
    if JOBS.set(tx).is_ok() {
        std::thread::spawn(move || worker(rx, events));
    }
}

/// Applies `command` on the DLL thread and waits for the result.
pub async fn run(command: Command) -> Result<()> {
    let (tx, rx) = oneshot::channel();
    let job: Job = Box::new(move |link: &mut Link| {
        tx.send(link.apply(command)).ok();
    });
    JOBS.get()
        .context("VoiceMeeter-Thread läuft nicht")?
        .send(job)
        .map_err(|_| anyhow!("VoiceMeeter-Thread beendet"))?;
    rx.await.context("VoiceMeeter-Thread antwortet nicht")?
}

fn worker(jobs: mpsc::Receiver<Job>, events: UnboundedSender<Event>) {
    let mut link = Link::new(events);
    loop {
        match jobs.recv_timeout(POLL) {
            Ok(job) => job(&mut link),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        link.tick();
    }
}

/// Connection state, owned by the DLL thread.
struct Link {
    events: UnboundedSender<Event>,
    remote: Option<Remote>,
    next_attempt: Instant,
    reported: bool,
}

impl Link {
    fn new(events: UnboundedSender<Event>) -> Self {
        Self {
            events,
            remote: None,
            next_attempt: Instant::now(),
            reported: false,
        }
    }

    fn apply(&self, command: Command) -> Result<()> {
        let remote = self.remote.as_ref().context(OFFLINE)?;
        match command {
            Command::Set(name, value) => remote.set(&name, value),
            Command::Script(text) => remote.script(&text),
        }
    }

    fn tick(&mut self) {
        match self.remote.as_ref().map(Remote::poll) {
            Some(Ok(Some(snapshot))) => self.send(Event::Snapshot(snapshot)),
            Some(Ok(None)) => {}
            Some(Err(err)) => self.fail(err),
            None if Instant::now() >= self.next_attempt => self.connect(),
            None => {}
        }
    }

    fn connect(&mut self) {
        match Remote::connect().and_then(|remote| remote.snapshot().map(|s| (remote, s))) {
            Ok((remote, snapshot)) => {
                self.remote = Some(remote);
                self.reported = false;
                self.send(Event::Snapshot(snapshot));
            }
            Err(err) => self.fail(err),
        }
    }

    /// Drops the connection and retries later. The reason is reported once per outage.
    fn fail(&mut self, err: anyhow::Error) {
        self.remote = None;
        self.next_attempt = Instant::now() + RETRY;
        if !self.reported {
            self.reported = true;
            self.send(Event::Offline(format!("{err:#}")));
        }
    }

    fn send(&self, event: Event) {
        self.events.send(event).ok();
    }
}

/// A logged-in connection to VoiceMeeter. Dropping it logs out.
struct Remote {
    lib: Library,
    layout: Layout,
}

impl Remote {
    fn connect() -> Result<Self> {
        let lib = unsafe { Library::new(dll_path()) }.context(NOT_INSTALLED)?;
        let login: Status = symbol(&lib, b"VBVMR_Login\0")?;
        // 1 means logged in, but VoiceMeeter is not running. That session must be
        // closed, or every later login fails with -2 ("already logged in").
        let code = unsafe { login() };
        if code > 0 {
            logout(&lib);
        }
        ensure!(code == 0, OFFLINE);
        let layout = read_layout(&lib).inspect_err(|_| logout(&lib))?;
        Ok(Self { lib, layout })
    }

    /// `VBVMR_IsParametersDirty` is 1 after a change; the values are read only then.
    fn poll(&self) -> Result<Option<Snapshot>> {
        let dirty: Status = symbol(&self.lib, b"VBVMR_IsParametersDirty\0")?;
        let code = unsafe { dirty() };
        ensure!(code >= 0, "VoiceMeeter-Fehler {code}");
        if code > 0 {
            self.snapshot().map(Some)
        } else {
            Ok(None)
        }
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let mut values = HashMap::new();
        for name in self.layout.param_names() {
            let value = self.get(&name)?;
            values.insert(name, value);
        }
        Ok(Snapshot {
            layout: self.layout,
            values,
        })
    }

    fn get(&self, name: &str) -> Result<f32> {
        let function: GetFloat = symbol(&self.lib, b"VBVMR_GetParameterFloat\0")?;
        let c_name = CString::new(name)?;
        let mut value = 0.0;
        let code = unsafe { function(c_name.as_ptr(), &mut value) };
        ensure!(code == 0, "VoiceMeeter-Fehler {code} bei {name}");
        Ok(value)
    }

    fn set(&self, name: &str, value: f32) -> Result<()> {
        let function: SetFloat = symbol(&self.lib, b"VBVMR_SetParameterFloat\0")?;
        let c_name = CString::new(name)?;
        let code = unsafe { function(c_name.as_ptr(), value) };
        ensure!(code == 0, "VoiceMeeter-Fehler {code} bei {name}");
        Ok(())
    }

    fn script(&self, text: &str) -> Result<()> {
        let function: SetScript = symbol(&self.lib, b"VBVMR_SetParameters\0")?;
        let c_text = CString::new(text).context("Befehl enthält ein Null-Zeichen")?;
        let code = unsafe { function(c_text.as_ptr()) };
        ensure!(code == 0, "VoiceMeeter lehnt den Befehl ab ({code})");
        Ok(())
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        logout(&self.lib);
    }
}

fn logout(lib: &Library) {
    if let Ok(function) = symbol::<Status>(lib, b"VBVMR_Logout\0") {
        unsafe { function() };
    }
}

fn read_layout(lib: &Library) -> Result<Layout> {
    let get_type: GetType = symbol(lib, b"VBVMR_GetVoicemeeterType\0")?;
    let mut kind: c_long = 0;
    ensure!(
        unsafe { get_type(&mut kind) } == 0,
        "VoiceMeeter-Typ nicht lesbar"
    );
    Layout::from_type(kind.into()).with_context(|| format!("unbekannter VoiceMeeter-Typ {kind}"))
}

/// Looks up one function. `F` must match the signature in VoicemeeterRemote.h.
fn symbol<F: Copy>(lib: &Library, name: &[u8]) -> Result<F> {
    let function = unsafe { lib.get::<F>(name) }?;
    Ok(*function)
}

/// `VoicemeeterRemote64.dll` in the install folder from the uninstall entry, or the default folder.
fn dll_path() -> PathBuf {
    let dir = uninstall_string()
        .as_deref()
        .and_then(install_dir)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR));
    dir.join("VoicemeeterRemote64.dll")
}

fn uninstall_string() -> Option<String> {
    let mut buf = [0u16; 512];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(
                r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\VB:Voicemeeter {17359A74-1236-5467}"
            ),
            w!("UninstallString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast::<c_void>()),
            Some(&mut bytes as *mut u32),
        )
    };
    (status == ERROR_SUCCESS).then(|| {
        // `bytes` counts the terminating NUL as well.
        let units = (bytes as usize / 2).saturating_sub(1).min(buf.len());
        String::from_utf16_lossy(&buf[..units])
    })
}

/// Folder of the program in an uninstall string such as `"C:\...\uninstall.exe" -uninstall`.
fn install_dir(uninstall: &str) -> Option<PathBuf> {
    let uninstall = uninstall.trim();
    let program = match uninstall.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => uninstall,
    };
    Path::new(program)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_from_uninstall_string() {
        assert_eq!(
            install_dir(r#""C:\Program Files (x86)\VB\Voicemeeter\uninstall.exe" -uninstall"#),
            Some(PathBuf::from(r"C:\Program Files (x86)\VB\Voicemeeter"))
        );
        assert_eq!(
            install_dir(r"C:\VB\Voicemeeter\uninstall.exe"),
            Some(PathBuf::from(r"C:\VB\Voicemeeter"))
        );
        assert_eq!(install_dir("uninstall.exe"), None);
    }
}

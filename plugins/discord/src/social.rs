//! Online status via the Discord Social SDK. The local RPC interface cannot
//! read or set it. The SDK is proprietary, so it is not linked but loaded at
//! runtime from next to the plugin binary; without it, only the status action
//! fails.
//!
//! A worker thread keeps one SDK connection open so status keys follow
//! changes made in Discord itself (reconnecting per poll would hit Discord's
//! connection limits). The worker lives as long as the process: the SDK keeps
//! callback pointers and runs callbacks of every client from any thread that
//! calls `Discord_RunCallbacks`, so tearing a worker down and starting another
//! one crashed the plugin.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use libloading::Library;

#[cfg(windows)]
const LIB: &str = "discord_partner_sdk.dll";
#[cfg(not(windows))]
const LIB: &str = "libdiscord_partner_sdk.so";

pub const SCOPE: &str = "sdk.social_layer_presence";

pub const ONLINE: i32 = 0;
pub const IDLE: i32 = 3;
pub const DND: i32 = 4;
pub const INVISIBLE: i32 = 5;

/// `online`, `idle`, `dnd` or `invisible`.
pub fn status_code(name: &str) -> Option<i32> {
    Some(match name {
        "online" => ONLINE,
        "idle" => IDLE,
        "dnd" => DND,
        "invisible" => INVISIBLE,
        _ => return None,
    })
}

/// Status a key press sets: `primary`, or `alternate` while `primary` is
/// already active.
pub fn press_target(primary: i32, alternate: Option<i32>, current: Option<i32>) -> i32 {
    match alternate {
        Some(alternate) if current == Some(primary) => alternate,
        _ => primary,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DStr {
    ptr: *const u8,
    size: usize,
}

impl DStr {
    fn of(s: &str) -> Self {
        Self {
            ptr: s.as_ptr(),
            size: s.len(),
        }
    }

    fn text(self) -> String {
        if self.ptr.is_null() {
            return String::new();
        }
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.ptr, self.size) })
            .into_owned()
    }
}

#[repr(C)]
struct Handle(*mut c_void);

impl Handle {
    fn null() -> Self {
        Self(std::ptr::null_mut())
    }
}

type Free = Option<unsafe extern "C" fn(*mut c_void)>;
type ResultCb = unsafe extern "C" fn(*mut Handle, *mut c_void);
type StatusCb = unsafe extern "C" fn(i32, i32, i32, *mut c_void);
type UserCb = unsafe extern "C" fn(u64, *mut c_void);

struct Api {
    run_callbacks: unsafe extern "C" fn(),
    init: unsafe extern "C" fn(*mut Handle),
    drop: unsafe extern "C" fn(*mut Handle),
    on_status: unsafe extern "C" fn(*mut Handle, StatusCb, Free, *mut c_void),
    on_user: unsafe extern "C" fn(*mut Handle, UserCb, Free, *mut c_void),
    update_token: unsafe extern "C" fn(*mut Handle, i32, DStr, ResultCb, Free, *mut c_void),
    connect: unsafe extern "C" fn(*mut Handle),
    disconnect: unsafe extern "C" fn(*mut Handle),
    set_online_status: unsafe extern "C" fn(*mut Handle, i32, ResultCb, Free, *mut c_void),
    current_user: unsafe extern "C" fn(*mut Handle, *mut Handle),
    user_id: unsafe extern "C" fn(*mut Handle) -> u64,
    user_status: unsafe extern "C" fn(*mut Handle) -> i32,
    user_drop: unsafe extern "C" fn(*mut Handle),
    successful: unsafe extern "C" fn(*mut Handle) -> bool,
    to_string: unsafe extern "C" fn(*mut Handle, *mut DStr),
}

/// The library stays loaded: the SDK starts threads that must not outlive it.
fn api() -> anyhow::Result<&'static Api> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| load().map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| anyhow!("{e}"))
}

fn load() -> anyhow::Result<Api> {
    let path = std::env::current_exe()?.with_file_name(LIB);
    let lib = unsafe { Library::new(&path) }
        .with_context(|| format!("Discord Social SDK fehlt ({})", path.display()))?;
    let lib: &'static Library = Box::leak(Box::new(lib));
    macro_rules! sym {
        ($name:literal) => {
            *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.context($name)?
        };
    }
    Ok(Api {
        run_callbacks: sym!("Discord_RunCallbacks"),
        init: sym!("Discord_Client_Init"),
        drop: sym!("Discord_Client_Drop"),
        on_status: sym!("Discord_Client_SetStatusChangedCallback"),
        on_user: sym!("Discord_Client_SetUserUpdatedCallback"),
        update_token: sym!("Discord_Client_UpdateToken"),
        connect: sym!("Discord_Client_Connect"),
        disconnect: sym!("Discord_Client_Disconnect"),
        set_online_status: sym!("Discord_Client_SetOnlineStatus"),
        current_user: sym!("Discord_Client_GetCurrentUser"),
        user_id: sym!("Discord_UserHandle_Id"),
        user_status: sym!("Discord_UserHandle_Status"),
        user_drop: sym!("Discord_UserHandle_Drop"),
        successful: sym!("Discord_ClientResult_Successful"),
        to_string: sym!("Discord_ClientResult_ToString"),
    })
}

/// Reported to the plugin.
pub enum Event {
    Status(i32),
    Failed(String),
}

enum Cmd {
    Token(String),
    Set(i32),
    Stop,
}

/// Handle to the worker; `stop` disconnects, `set_token` connects again.
pub struct Social {
    cmd: Sender<Cmd>,
}

impl Social {
    pub fn start(token: String, report: impl Fn(Event) + Send + 'static) -> Self {
        let (cmd, rx) = mpsc::channel();
        std::thread::spawn(move || match api() {
            Ok(api) => Worker::new(api, report).run(token, rx),
            Err(err) => report(Event::Failed(format!("{err:#}"))),
        });
        Self { cmd }
    }

    pub fn set_token(&self, token: String) {
        self.cmd.send(Cmd::Token(token)).ok();
    }

    pub fn set_status(&self, status: i32) {
        self.cmd.send(Cmd::Set(status)).ok();
    }

    pub fn stop(&self) {
        self.cmd.send(Cmd::Stop).ok();
    }
}

/// Raised by SDK callbacks, which run inside `Discord_RunCallbacks` on the
/// worker thread.
enum Sdk {
    TokenSet,
    Ready,
    Disconnected,
    UserUpdated(u64),
    StatusSet,
    Failed(String),
}

struct Callbacks {
    api: &'static Api,
    tx: Sender<Sdk>,
}

impl Callbacks {
    unsafe fn from<'a>(user_data: *mut c_void) -> &'a Self {
        unsafe { &*(user_data as *const Self) }
    }

    fn result(&self, result: *mut Handle, what: &str, ok: Sdk) {
        let event = if unsafe { (self.api.successful)(result) } {
            ok
        } else {
            let mut text = DStr {
                ptr: std::ptr::null(),
                size: 0,
            };
            unsafe { (self.api.to_string)(result, &mut text) };
            Sdk::Failed(format!("{what}: {}", text.text()))
        };
        self.tx.send(event).ok();
    }
}

unsafe extern "C" fn on_token(result: *mut Handle, user_data: *mut c_void) {
    unsafe { Callbacks::from(user_data) }.result(result, "Token", Sdk::TokenSet);
}

unsafe extern "C" fn on_set(result: *mut Handle, user_data: *mut c_void) {
    unsafe { Callbacks::from(user_data) }.result(result, "Status", Sdk::StatusSet);
}

unsafe extern "C" fn on_status(status: i32, error: i32, detail: i32, user_data: *mut c_void) {
    const DISCONNECTED: i32 = 0;
    const READY: i32 = 3;
    let event = match status {
        READY => Sdk::Ready,
        DISCONNECTED if error != 0 => {
            Sdk::Failed(format!("Verbindung abgelehnt (Fehler {error}/{detail})"))
        }
        DISCONNECTED => Sdk::Disconnected,
        _ => return,
    };
    unsafe { Callbacks::from(user_data) }.tx.send(event).ok();
}

unsafe extern "C" fn on_user(user: u64, user_data: *mut c_void) {
    unsafe { Callbacks::from(user_data) }
        .tx
        .send(Sdk::UserUpdated(user))
        .ok();
}

struct Worker<R> {
    api: &'static Api,
    report: R,
    client: Handle,
    /// Boxed so the pointer handed to the SDK stays valid.
    callbacks: Box<Callbacks>,
    sdk: mpsc::Receiver<Sdk>,
    ready: bool,
    connecting: bool,
    pending: Option<i32>,
    requested: Option<i32>,
    reported: Option<i32>,
}

impl<R: Fn(Event)> Worker<R> {
    fn new(api: &'static Api, report: R) -> Self {
        let (tx, sdk) = mpsc::channel();
        let mut worker = Self {
            api,
            report,
            client: Handle::null(),
            callbacks: Box::new(Callbacks { api, tx }),
            sdk,
            ready: false,
            connecting: false,
            pending: None,
            requested: None,
            reported: None,
        };
        let data = worker.user_data();
        unsafe {
            (api.init)(&mut worker.client);
            (api.on_status)(&mut worker.client, on_status, None, data);
            (api.on_user)(&mut worker.client, on_user, None, data);
        }
        worker
    }

    fn user_data(&self) -> *mut c_void {
        &*self.callbacks as *const Callbacks as *mut c_void
    }

    fn run(mut self, mut token: String, rx: mpsc::Receiver<Cmd>) {
        self.sign_in(&token);
        let mut active = true;
        let mut polled = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Cmd::Token(fresh)) if fresh != token || !active => {
                    token = fresh;
                    if active {
                        unsafe { (self.api.disconnect)(&mut self.client) };
                    }
                    active = true;
                    self.sign_in(&token);
                }
                Ok(Cmd::Token(_)) => {}
                Ok(Cmd::Stop) if active => {
                    active = false;
                    self.ready = false;
                    self.connecting = false;
                    self.pending = None;
                    self.reported = None;
                    unsafe { (self.api.disconnect)(&mut self.client) };
                }
                Ok(Cmd::Stop) => {}
                Ok(Cmd::Set(_)) if !active => {}
                Ok(Cmd::Set(status)) if self.ready => self.set(status),
                Ok(Cmd::Set(status)) => {
                    self.pending = Some(status);
                    if !self.connecting {
                        self.sign_in(&token);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            unsafe { (self.api.run_callbacks)() };
            while let Ok(event) = self.sdk.try_recv() {
                self.on_sdk(event);
            }
            // Changes made in Discord itself do not always raise
            // `UserUpdated`; the SDK's cached user is cheap to read.
            if polled.elapsed() >= Duration::from_secs(2) {
                polled = Instant::now();
                self.report_current(None);
            }
        }
        unsafe {
            (self.api.disconnect)(&mut self.client);
            (self.api.run_callbacks)();
            (self.api.drop)(&mut self.client);
        }
    }

    fn sign_in(&mut self, token: &str) {
        const BEARER: i32 = 1;
        self.ready = false;
        self.connecting = true;
        let data = self.user_data();
        unsafe {
            (self.api.update_token)(
                &mut self.client,
                BEARER,
                DStr::of(token),
                on_token,
                None,
                data,
            )
        };
    }

    fn set(&mut self, status: i32) {
        self.requested = Some(status);
        let data = self.user_data();
        unsafe { (self.api.set_online_status)(&mut self.client, status, on_set, None, data) };
    }

    fn on_sdk(&mut self, event: Sdk) {
        match event {
            Sdk::TokenSet => unsafe { (self.api.connect)(&mut self.client) },
            Sdk::Ready => {
                self.ready = true;
                self.connecting = false;
                match self.pending.take() {
                    Some(status) => self.set(status),
                    None => self.report_current(None),
                }
            }
            Sdk::Disconnected => {
                self.ready = false;
                self.connecting = false;
            }
            Sdk::UserUpdated(user) => self.report_current(Some(user)),
            Sdk::StatusSet => {
                if let Some(status) = self.requested.take() {
                    self.report(status);
                }
            }
            Sdk::Failed(err) => {
                self.ready = false;
                self.reported = None;
                self.connecting = false;
                self.pending = None;
                (self.report)(Event::Failed(err));
            }
        }
    }

    /// Reports the signed-in user's status; `only_for` skips updates about
    /// other users (friends).
    fn report_current(&mut self, only_for: Option<u64>) {
        if !self.ready {
            return;
        }
        let mut user = Handle::null();
        unsafe { (self.api.current_user)(&mut self.client, &mut user) };
        if user.0.is_null() {
            return;
        }
        let (id, status) = unsafe {
            (
                (self.api.user_id)(&mut user),
                (self.api.user_status)(&mut user),
            )
        };
        unsafe { (self.api.user_drop)(&mut user) };
        if only_for.is_none_or(|u| u == id) {
            self.report(status);
        }
    }

    fn report(&mut self, status: i32) {
        if self.reported != Some(status) {
            self.reported = Some(status);
            (self.report)(Event::Status(status));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names() {
        assert_eq!(status_code("dnd"), Some(DND));
        assert_eq!(status_code("invisible"), Some(INVISIBLE));
        assert_eq!(status_code("busy"), None);
    }

    #[test]
    fn press_toggles_only_from_primary() {
        assert_eq!(
            press_target(ONLINE, Some(INVISIBLE), Some(ONLINE)),
            INVISIBLE
        );
        assert_eq!(
            press_target(ONLINE, Some(INVISIBLE), Some(INVISIBLE)),
            ONLINE
        );
        assert_eq!(press_target(ONLINE, Some(INVISIBLE), Some(DND)), ONLINE);
        assert_eq!(press_target(ONLINE, Some(INVISIBLE), None), ONLINE);
        assert_eq!(press_target(DND, None, Some(DND)), DND);
    }
}

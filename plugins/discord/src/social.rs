//! Online status via the Discord Social SDK. The local RPC interface cannot
//! set it. The SDK is proprietary, so it is not linked but loaded at runtime
//! from next to the plugin binary; without it, only this action fails.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use libloading::Library;

#[cfg(windows)]
const LIB: &str = "discord_partner_sdk.dll";
#[cfg(not(windows))]
const LIB: &str = "libdiscord_partner_sdk.so";

pub const SCOPE: &str = "sdk.social_layer_presence";

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

type Free = Option<unsafe extern "C" fn(*mut c_void)>;
type ResultCb = unsafe extern "C" fn(*mut Handle, *mut c_void);
type StatusCb = unsafe extern "C" fn(i32, i32, i32, *mut c_void);

struct Api {
    run_callbacks: unsafe extern "C" fn(),
    init: unsafe extern "C" fn(*mut Handle),
    drop: unsafe extern "C" fn(*mut Handle),
    on_status: unsafe extern "C" fn(*mut Handle, StatusCb, Free, *mut c_void),
    update_token: unsafe extern "C" fn(*mut Handle, i32, DStr, ResultCb, Free, *mut c_void),
    connect: unsafe extern "C" fn(*mut Handle),
    disconnect: unsafe extern "C" fn(*mut Handle),
    set_online_status: unsafe extern "C" fn(*mut Handle, i32, ResultCb, Free, *mut c_void),
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
        update_token: sym!("Discord_Client_UpdateToken"),
        connect: sym!("Discord_Client_Connect"),
        disconnect: sym!("Discord_Client_Disconnect"),
        set_online_status: sym!("Discord_Client_SetOnlineStatus"),
        successful: sym!("Discord_ClientResult_Successful"),
        to_string: sym!("Discord_ClientResult_ToString"),
    })
}

enum Event {
    TokenSet,
    Ready,
    StatusSet,
    Failed(String),
}

/// SDK callbacks run inside `Discord_RunCallbacks` on the calling thread and
/// report here; `BUSY` keeps concurrent key presses from mixing their events.
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static BUSY: Mutex<()> = Mutex::new(());

fn push(event: Event) {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

fn check(result: *mut Handle, what: &str, ok: Event) {
    let Ok(api) = api() else { return };
    if unsafe { (api.successful)(result) } {
        push(ok);
    } else {
        let mut text = DStr {
            ptr: std::ptr::null(),
            size: 0,
        };
        unsafe { (api.to_string)(result, &mut text) };
        push(Event::Failed(format!("{what}: {}", text.text())));
    }
}

unsafe extern "C" fn on_token(result: *mut Handle, _: *mut c_void) {
    check(result, "Token", Event::TokenSet);
}

unsafe extern "C" fn on_set(result: *mut Handle, _: *mut c_void) {
    check(result, "Status", Event::StatusSet);
}

unsafe extern "C" fn on_status(status: i32, error: i32, detail: i32, _: *mut c_void) {
    const READY: i32 = 3;
    const DISCONNECTED: i32 = 0;
    if status == READY {
        push(Event::Ready);
    } else if status == DISCONNECTED && error != 0 {
        push(Event::Failed(format!(
            "Verbindung abgelehnt (Fehler {error}/{detail})"
        )));
    }
}

/// `online`, `idle`, `dnd` or `invisible`.
pub fn status_code(name: &str) -> Option<i32> {
    Some(match name {
        "online" => 0,
        "idle" => 3,
        "dnd" => 4,
        "invisible" => 5,
        _ => return None,
    })
}

/// Connects with the plugin's OAuth2 token, sets the status and disconnects;
/// Discord keeps the status afterwards. Blocking.
pub fn set_online_status(access_token: &str, status: i32) -> anyhow::Result<()> {
    let api = api()?;
    let _busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    const BEARER: i32 = 1;
    let null = std::ptr::null_mut();
    let mut client = Handle(null);
    unsafe {
        (api.init)(&mut client);
        (api.on_status)(&mut client, on_status, None, null);
        (api.update_token)(
            &mut client,
            BEARER,
            DStr::of(access_token),
            on_token,
            None,
            null,
        );
    }
    let result = run(api, &mut client, status);
    unsafe {
        (api.disconnect)(&mut client);
        (api.run_callbacks)();
        (api.drop)(&mut client);
    }
    result
}

fn run(api: &Api, client: &mut Handle, status: i32) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        unsafe { (api.run_callbacks)() };
        let events = std::mem::take(&mut *EVENTS.lock().unwrap_or_else(|e| e.into_inner()));
        for event in events {
            match event {
                Event::TokenSet => unsafe { (api.connect)(client) },
                Event::Ready => unsafe {
                    (api.set_online_status)(client, status, on_set, None, std::ptr::null_mut())
                },
                Event::StatusSet => return Ok(()),
                Event::Failed(err) => bail!("{err}"),
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("Zeitüberschreitung beim Setzen des Status")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names() {
        assert_eq!(status_code("dnd"), Some(4));
        assert_eq!(status_code("invisible"), Some(5));
        assert_eq!(status_code("busy"), None);
    }
}

//! How long the user has not touched keyboard or mouse.
//!
//! - Windows: `GetLastInputInfo`.
//! - Linux/X11: the MIT-SCREEN-SAVER extension (same X connection type `enigo` uses).
//!   Wayland sessions report only X11 clients' input, so they are unsupported.

use std::time::Duration;

use tokio::sync::mpsc;

const POLL: Duration = Duration::from_secs(2);

/// Sends the current idle time every few seconds; sends nothing where unsupported.
pub fn spawn() -> mpsc::Receiver<Duration> {
    let (tx, rx) = mpsc::channel(1);
    std::thread::Builder::new()
        .name("opendeckn3-idle".into())
        .spawn(move || {
            let mut source = Source::open();
            loop {
                match source.as_mut().and_then(Source::idle) {
                    Some(idle) => {
                        if tx.blocking_send(idle).is_err() {
                            return;
                        }
                    }
                    None => {
                        tracing::warn!("idle time unavailable, dimming the deck when idle is off");
                        return;
                    }
                }
                std::thread::sleep(POLL);
            }
        })
        .expect("cannot spawn idle thread");
    rx
}

#[cfg(windows)]
struct Source;

#[cfg(windows)]
impl Source {
    fn open() -> Option<Self> {
        Some(Self)
    }

    fn idle(&mut self) -> Option<Duration> {
        use windows::Win32::{
            System::SystemInformation::GetTickCount,
            UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
        };
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a valid, correctly sized LASTINPUTINFO.
        unsafe { GetLastInputInfo(&mut info) }.as_bool().then(|| {
            // Both are 32-bit tick counts; wrapping_sub survives the 49-day rollover.
            let now = unsafe { GetTickCount() };
            Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime)))
        })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
struct Source {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
}

#[cfg(all(unix, not(target_os = "macos")))]
impl Source {
    fn open() -> Option<Self> {
        use x11rb::connection::Connection;
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland") {
            return None;
        }
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some(Self { conn, root })
    }

    fn idle(&mut self) -> Option<Duration> {
        use x11rb::protocol::screensaver::ConnectionExt;
        let reply = self
            .conn
            .screensaver_query_info(self.root)
            .ok()?
            .reply()
            .ok()?;
        Some(Duration::from_millis(u64::from(reply.ms_since_user_input)))
    }
}

#[cfg(target_os = "macos")]
struct Source;

#[cfg(target_os = "macos")]
impl Source {
    fn open() -> Option<Self> {
        None
    }

    fn idle(&mut self) -> Option<Duration> {
        None
    }
}

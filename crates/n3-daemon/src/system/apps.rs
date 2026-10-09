//! Installed applications for the "Programm öffnen" dropdown.
//!
//! - Windows: shortcuts of the start menu (all users + current user), including
//!   game launcher links (`.url` with `steam://` etc.).
//! - Linux: `.desktop` entries of the application menu.
//! - macOS: `.app` bundles in `/Applications` and `~/Applications`.
//!
//! Each entry is `(path, name)`; `launch` can open every path.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Sorted by name, without duplicates and uninstallers.
pub fn installed_apps() -> Vec<(String, String)> {
    let mut apps: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (path, name) in scan() {
        let lower = name.to_lowercase();
        if [
            "uninstall",
            "deinstall",
            "entfernen",
            "readme",
            "liesmich",
            "website",
            "help",
            "hilfe",
        ]
        .iter()
        .any(|w| lower.contains(w))
        {
            continue;
        }
        apps.entry(lower).or_insert((path, name));
    }
    apps.into_values().collect()
}

#[cfg(windows)]
fn scan() -> Vec<(String, String)> {
    let mut roots = Vec::new();
    if let Some(data) = std::env::var_os("ProgramData") {
        roots.push(PathBuf::from(data).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let mut found = Vec::new();
    for root in roots {
        walk(&root, 4, &mut |path| {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if ext.eq_ignore_ascii_case("lnk")
                || (ext.eq_ignore_ascii_case("url") && is_launcher_link(path))
            {
                found.push((path.display().to_string(), stem(path)));
            }
        });
    }
    found
}

/// Internet shortcuts with a non-web scheme, e.g. `steam://rungameid/…` from
/// Steam or other game launchers. Plain web links stay out of the app list.
#[cfg(windows)]
fn is_launcher_link(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.lines()
        .find_map(|l| l.trim().strip_prefix("URL="))
        .is_some_and(|url| url.contains("://") && !url.to_ascii_lowercase().starts_with("http"))
}

/// The icon the Windows shell shows for a program, shortcut or file.
#[cfg(windows)]
pub fn shell_icon(path: &str) -> Option<image::RgbaImage> {
    use windows::{
        Win32::{
            Foundation::SIZE,
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
                GetDIBits, ReleaseDC,
            },
            System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
            UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_ICONONLY},
        },
        core::HSTRING,
    };
    const PX: i32 = 256;
    // SAFETY: plain Win32 calls; the bitmap is owned here and deleted after copying.
    unsafe {
        // Shell items need COM on this thread; "already initialized" is fine.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // The shell only parses backslash paths.
        let path = HSTRING::from(path.replace('/', "\\"));
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&path, None).ok()?;
        let bitmap = factory
            .GetImage(SIZE { cx: PX, cy: PX }, SIIGBF_ICONONLY)
            .ok()?;
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: PX,
                biHeight: -PX, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (PX * PX * 4) as usize];
        let dc = GetDC(None);
        let rows = GetDIBits(
            dc,
            bitmap,
            0,
            PX as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        let _ = DeleteObject(bitmap.into());
        if rows == 0 {
            return None;
        }
        bgra_to_rgba(&mut pixels);
        image::RgbaImage::from_raw(PX as u32, PX as u32, pixels)
    }
}

/// Shell bitmaps are premultiplied BGRA; old icons without alpha come back fully transparent.
#[cfg(any(windows, test))]
fn bgra_to_rgba(pixels: &mut [u8]) {
    let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
    for p in pixels.chunks_exact_mut(4) {
        p.swap(0, 2);
        if !has_alpha {
            p[3] = 255;
        } else if p[3] != 0 && p[3] != 255 {
            let alpha = u16::from(p[3]);
            for c in &mut p[..3] {
                *c = (u16::from(*c) * 255 / alpha).min(255) as u8;
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn scan() -> Vec<(String, String)> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }
    let mut found = Vec::new();
    for root in roots {
        // Bundles are directories: look one level into sub-folders like "Utilities".
        for entry in read_dir(&root) {
            if entry.extension().is_some_and(|e| e == "app") {
                found.push((entry.display().to_string(), stem(&entry)));
            } else if entry.is_dir() {
                for inner in read_dir(&entry) {
                    if inner.extension().is_some_and(|e| e == "app") {
                        found.push((inner.display().to_string(), stem(&inner)));
                    }
                }
            }
        }
    }
    found
}

#[cfg(all(unix, not(target_os = "macos")))]
fn scan() -> Vec<(String, String)> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(data) = dirs::data_dir() {
        roots.push(data.join("applications"));
    }
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    roots.extend(
        system
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join("applications")),
    );
    roots.push("/var/lib/flatpak/exports/share/applications".into());
    let mut found = Vec::new();
    for root in roots {
        walk(&root, 2, &mut |path| {
            if path.extension().is_some_and(|e| e == "desktop")
                && let Some(name) = desktop_name(path)
            {
                found.push((path.display().to_string(), name));
            }
        });
    }
    found
}

/// Display name of a launchable `.desktop` entry (localized if available).
#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_name(path: &Path) -> Option<String> {
    let entry = DesktopEntry::read(path)?;
    (entry.kind == "Application" && !entry.hidden && !entry.exec.is_empty()).then_some(entry.name)
}

#[derive(Debug, Default)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub kind: String,
    pub hidden: bool,
}

impl DesktopEntry {
    pub fn read(path: &Path) -> Option<Self> {
        Some(Self::parse(&std::fs::read_to_string(path).ok()?))
    }

    pub fn parse(text: &str) -> Self {
        let lang = std::env::var("LANG").unwrap_or_default();
        let lang = lang.split(['_', '.']).next().unwrap_or_default().to_owned();
        let mut entry = Self::default();
        let mut localized = None;
        let mut in_main = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "Name" => entry.name = value.to_owned(),
                "Exec" => entry.exec = value.to_owned(),
                "Type" => entry.kind = value.to_owned(),
                "NoDisplay" | "Hidden" if value == "true" => entry.hidden = true,
                _ if !lang.is_empty() && key == format!("Name[{lang}]") => {
                    localized = Some(value.to_owned())
                }
                _ => {}
            }
        }
        if let Some(name) = localized {
            entry.name = name;
        }
        entry
    }

    /// `Exec` without field codes (`%U`, `%f` …), split into program + args.
    #[cfg(any(test, all(unix, not(target_os = "macos"))))]
    pub fn command(&self) -> anyhow::Result<Vec<String>> {
        let words = shell_words::split(&self.exec)?;
        let words: Vec<String> = words
            .into_iter()
            .filter(|w| !(w.len() == 2 && w.starts_with('%')))
            .collect();
        anyhow::ensure!(!words.is_empty(), "Eintrag ohne Programm");
        Ok(words)
    }
}

#[allow(dead_code)] // Windows and macOS
fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[allow(dead_code)] // macOS only
fn read_dir(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

#[allow(dead_code)] // Windows and Linux
fn walk(dir: &Path, depth: u8, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, visit);
            }
        } else {
            visit(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_entries() {
        let entry = DesktopEntry::parse(
            "[Desktop Entry]\nType=Application\nName=Firefox Web Browser\nExec=firefox --new-window %u\n\n[Desktop Action new]\nName=Neues Fenster\nExec=firefox -x\n",
        );
        assert_eq!(entry.kind, "Application");
        assert_eq!(
            entry.exec, "firefox --new-window %u",
            "actions do not override the main entry"
        );
        assert_eq!(entry.command().unwrap(), ["firefox", "--new-window"]);
        assert!(!entry.hidden);
        assert!(DesktopEntry::parse("[Desktop Entry]\nNoDisplay=true\n").hidden);
    }

    #[test]
    fn converts_shell_bitmaps() {
        // Premultiplied half-transparent blue, then an icon without any alpha.
        let mut px = vec![128, 0, 0, 128];
        bgra_to_rgba(&mut px);
        assert_eq!(px, [0, 0, 255, 128]);
        let mut px = vec![10, 20, 30, 0];
        bgra_to_rgba(&mut px);
        assert_eq!(px, [30, 20, 10, 255]);
    }

    #[test]
    fn lists_apps_without_panicking() {
        // Content depends on the machine; just make sure scanning works and is sorted.
        let apps = installed_apps();
        let names: Vec<String> = apps.iter().map(|(_, n)| n.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(!names.iter().any(|n| n.contains("uninstall")));
    }
}

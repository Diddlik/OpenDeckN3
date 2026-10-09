//! Installed Steam games, read from Steam's own library files so games
//! without a start-menu shortcut show up too. Launched via `steam://rungameid/<id>`.

use std::path::PathBuf;

use image::DynamicImage;

pub const RUN_PREFIX: &str = "steam://rungameid/";

/// `(steam://rungameid/<id>, name)` of every fully installed game.
pub fn games() -> Vec<(String, String)> {
    let mut games = Vec::new();
    for library in libraries() {
        let Ok(entries) = std::fs::read_dir(library.join("steamapps")) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Some(game) = game(&text) {
                games.push(game);
            }
        }
    }
    games
}

fn game(manifest: &str) -> Option<(String, String)> {
    let id = value(manifest, "appid")?;
    let name = value(manifest, "name")?;
    let flags: u32 = value(manifest, "StateFlags")?.parse().ok()?;
    // Bit 4 = fully installed. Runtimes and redistributables are not games.
    let tool = ["Steamworks", "Proton", "Steam Linux Runtime"]
        .iter()
        .any(|t| name.starts_with(t));
    (flags & 4 != 0 && !tool).then(|| (format!("{RUN_PREFIX}{id}"), name))
}

pub fn name(id: &str) -> Option<String> {
    libraries().into_iter().find_map(|library| {
        let manifest = library.join(format!("steamapps/appmanifest_{id}.acf"));
        value(&std::fs::read_to_string(manifest).ok()?, "name")
    })
}

/// Square crop of the game's cover from Steam's library cache, else its small icon.
pub fn icon(id: &str) -> Option<DynamicImage> {
    let dir = root()?.join("appcache/librarycache").join(id);
    let entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    // Newer Steam versions keep each image in a hash-named sub-folder.
    let folders = std::iter::once(&dir).chain(entries.iter().filter(|p| p.is_dir()));
    let cover = folders
        .flat_map(|d| ["library_capsule.jpg", "library_600x900.jpg"].map(|f| d.join(f)))
        .find(|p| p.is_file());
    // The small icon sits next to the folders, named by its hash.
    let small_icon = || {
        entries.iter().find(|p| {
            p.extension().is_some_and(|e| e == "jpg")
                && p.file_stem().is_some_and(|s| s.len() == 40)
        })
    };
    // Steam saves some PNGs with a .jpg name, so detect the format from the content.
    let open = |path: &PathBuf| {
        image::ImageReader::open(path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()
    };
    let cover = cover
        .and_then(|p| open(&p))
        .or_else(|| open(small_icon()?))?;
    let side = cover.width().min(cover.height());
    Some(cover.crop_imm(
        (cover.width() - side) / 2,
        (cover.height() - side) / 2,
        side,
        side,
    ))
}

fn libraries() -> Vec<PathBuf> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let folders =
        std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
    let mut libraries: Vec<PathBuf> = values(&folders, "path").map(PathBuf::from).collect();
    if libraries.is_empty() {
        libraries.push(root);
    }
    libraries
}

#[cfg(windows)]
fn root() -> Option<PathBuf> {
    use windows::{
        Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW},
        core::w,
    };
    let mut buf = [0u16; 1024];
    let mut len = size_of_val(&buf) as u32;
    // SAFETY: the buffer and its byte length are passed together.
    let from_registry = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Valve\Steam"),
            w!("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    }
    .is_ok()
    .then(|| {
        let chars = (len as usize / 2).saturating_sub(1); // without the trailing NUL
        PathBuf::from(String::from_utf16_lossy(&buf[..chars]))
    });
    from_registry
        .or_else(|| Some(PathBuf::from(std::env::var_os("ProgramFiles(x86)")?).join("Steam")))
        .filter(|p| p.is_dir())
}

#[cfg(not(windows))]
fn root() -> Option<PathBuf> {
    [
        dirs::data_dir().map(|d| d.join("Steam")),
        dirs::home_dir().map(|h| h.join(".steam/steam")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_dir())
}

/// Values of `"key"  "value"` lines in Steam's text VDF/ACF files.
fn values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = String> + 'a {
    text.lines().filter_map(move |line| {
        let mut parts = line.trim().split('"').filter(|p| !p.trim().is_empty());
        (parts.next()? == key).then(|| parts.next().map(|v| v.replace(r"\\", r"\")))?
    })
}

fn value(text: &str, key: &str) -> Option<String> {
    values(text, key).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_manifests() {
        let manifest = "\"AppState\"\n{\n\t\"appid\"\t\t\"714010\"\n\t\"name\"\t\t\"Aimlabs\"\n\t\"StateFlags\"\t\t\"4\"\n}\n";
        assert_eq!(
            game(manifest),
            Some(("steam://rungameid/714010".into(), "Aimlabs".into()))
        );
        assert_eq!(
            game(&manifest.replace("\"4\"", "\"2\"")),
            None,
            "not installed"
        );
        assert_eq!(
            game(&manifest.replace("Aimlabs", "Steamworks Common Redistributables")),
            None
        );
        let folders = "\"0\"\n{\n\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n}\n";
        assert_eq!(
            values(folders, "path").collect::<Vec<_>>(),
            [r"D:\SteamLibrary"]
        );
    }
}

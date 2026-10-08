//! Parses human-readable shortcuts like `Ctrl+Shift+M` or `Alt+F4 Enter`.
//!
//! A sequence is a whitespace-separated list of chords; a chord is a `+`
//! separated list of modifiers followed by one key. Names are case-insensitive
//! and accept English and German spellings (`Strg`, `Entf`, `Bild auf` …).

use enigo::Key;

#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub modifiers: Vec<Key>,
    pub key: Key,
}

fn modifier(name: &str) -> Option<Vec<Key>> {
    Some(match name {
        "ctrl" | "control" | "strg" => vec![Key::Control],
        "shift" | "umschalt" => vec![Key::Shift],
        "alt" => vec![Key::Alt],
        // AltGr is Ctrl+Alt on Windows.
        "altgr" => vec![Key::Control, Key::Alt],
        "win" | "windows" | "super" | "meta" | "cmd" => vec![Key::Meta],
        _ => return None,
    })
}

fn named_key(name: &str) -> Option<Key> {
    const F_KEYS: [Key; 20] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::F13,
        Key::F14,
        Key::F15,
        Key::F16,
        Key::F17,
        Key::F18,
        Key::F19,
        Key::F20,
    ];
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return F_KEYS.get(n.checked_sub(1)?).copied();
    }
    Some(match name {
        "enter" | "return" | "eingabe" => Key::Return,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" | "leertaste" | "leer" => Key::Space,
        "backspace" | "rücktaste" => Key::Backspace,
        "delete" | "del" | "entf" => Key::Delete,
        "home" | "pos1" => Key::Home,
        "end" | "ende" => Key::End,
        "pageup" | "pgup" | "bildauf" => Key::PageUp,
        "pagedown" | "pgdn" | "bildab" => Key::PageDown,
        "up" | "hoch" | "oben" => Key::UpArrow,
        "down" | "runter" | "unten" => Key::DownArrow,
        "left" | "links" => Key::LeftArrow,
        "right" | "rechts" => Key::RightArrow,
        "capslock" | "feststell" => Key::CapsLock,
        "plus" => Key::Unicode('+'),
        "minus" => Key::Unicode('-'),
        #[cfg(not(target_os = "macos"))]
        "insert" | "ins" | "einfg" => Key::Insert,
        #[cfg(not(target_os = "macos"))]
        "printscreen" | "print" | "druck" => Key::PrintScr,
        #[cfg(not(target_os = "macos"))]
        "pause" => Key::Pause,
        "volumeup" | "lauter" => Key::VolumeUp,
        "volumedown" | "leiser" => Key::VolumeDown,
        "volumemute" | "mute" | "stumm" => Key::VolumeMute,
        "playpause" | "play" => Key::MediaPlayPause,
        "next" | "nexttrack" => Key::MediaNextTrack,
        "prev" | "previous" | "prevtrack" => Key::MediaPrevTrack,
        _ => return None,
    })
}

fn parse_chord(chord: &str) -> Result<Chord, String> {
    let parts: Vec<&str> = chord.split('+').collect();
    let (last, mods) = parts.split_last().ok_or("leeres Tastenkürzel")?;
    let mut modifiers = Vec::new();
    for m in mods {
        let name = m.trim().to_lowercase();
        let keys = modifier(&name).ok_or_else(|| format!("unbekannte Zusatztaste „{m}“"))?;
        modifiers.extend(keys);
    }
    let raw = last.trim();
    let name = raw.to_lowercase().replace([' ', '_', '-'], "");
    let key = if let Some(key) = named_key(&name) {
        key
    } else {
        let mut chars = raw.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Key::Unicode(c.to_lowercase().next().unwrap_or(c)),
            _ if modifier(&name).is_some() => {
                // A lone modifier, e.g. "Win" opens the start menu.
                let mut keys = modifier(&name).unwrap_or_default();
                let key = keys.pop().ok_or("leeres Tastenkürzel")?;
                modifiers.extend(keys);
                key
            }
            _ => return Err(format!("unbekannte Taste „{raw}“")),
        }
    };
    Ok(Chord { modifiers, key })
}

/// Parses `"Ctrl+C Ctrl+V"` into two chords. Empty input yields no chords.
pub fn parse(sequence: &str) -> Result<Vec<Chord>, String> {
    sequence
        .split_whitespace()
        .map(parse_chord)
        .collect::<Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_shortcuts() {
        assert_eq!(
            parse("Ctrl+Shift+M").unwrap(),
            vec![Chord {
                modifiers: vec![Key::Control, Key::Shift],
                key: Key::Unicode('m')
            }]
        );
        assert_eq!(parse("alt+f4").unwrap()[0].key, Key::F4);
        assert_eq!(parse("Strg+Entf").unwrap()[0].key, Key::Delete);
        assert_eq!(parse("Win").unwrap()[0].key, Key::Meta);
        assert_eq!(parse("Ctrl+Plus").unwrap()[0].key, Key::Unicode('+'));
        assert_eq!(parse("Ctrl+C Ctrl+V").unwrap().len(), 2);
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("Ctrl+Foo").is_err());
        assert!(parse("Hyper+A").is_err());
        assert!(parse("F25").is_err());
    }
}

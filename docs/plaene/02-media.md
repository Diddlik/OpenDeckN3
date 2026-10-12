# 02 – Medien / Now Playing

Steuert die gerade laufende Wiedergabe (Spotify, Browser, VLC …) und zeigt Cover und Titel auf der Taste. Die
eingebaute Aktion „Medien“ drückt nur Medientasten; dieses Plugin kennt den Zustand.

- UUID `de.opendeckn3.media`, Ordner `plugins/media`, Win + Linux.
- Windows: Global System Media Transport Controls über das Crate `windows`
  (Features `Media_Control`, `Storage_Streams`, `Foundation`).
- Linux: MPRIS über D-Bus mit `zbus` (reines Rust, keine C-Bibliothek). Player = erster Dienst
  `org.mpris.MediaPlayer2.*`, der gerade spielt, sonst der erste gefundene.
- Cover: Bytes mit `image` (Workspace) dekodieren, auf 144×144 skalieren, als PNG-Data-URL (`base64`) per `setImage`
  senden. Ohne Cover `setImage` mit `null` (Standardbild aus dem Manifest).

## Aufbau

Plattformcode hinter einer kleinen Schnittstelle in `src/windows.rs` und `src/linux.rs`:

```rust
struct Snapshot { title: String, artist: String, playing: bool, cover: Option<Vec<u8>>, position: Option<f64>, length: Option<f64> }
async fn snapshot() -> anyhow::Result<Option<Snapshot>>;
async fn command(cmd: Command) -> anyhow::Result<()>; // PlayPause, Next, Previous, Seek(seconds)
```

Abfrage alle 1 s (einfacher als Ereignisse auf beiden Plattformen). Cover nur neu senden, wenn sich Titel oder
Cover-Bytes ändern (Hash), weil Bilder groß sind.

## Aktionen

| UUID-Suffix | Name | Controller | Verhalten |
| --- | --- | --- | --- |
| `playpause` | Wiedergabe/Pause | Keypad | Zustand 0 = pausiert, 1 = spielt; Cover optional (`showCover` ja/nein) |
| `next` / `previous` | Nächster / Vorheriger Titel | Keypad | – |
| `nowplaying` | Läuft gerade | Keypad, Encoder | Cover als Bild, Titel als Laufschrift-freier Kurztext (Titel, gekürzt). Taste = Play/Pause. Drehregler: `seek` (Standard, 5 s je Raste) oder `track` (Raste = nächster/vorheriger Titel), Drücken = Play/Pause |

## Tests

- Kürzen des Titels auf Tastenbreite (Zeichen, nicht Bytes).
- Auswahl des MPRIS-Players (spielend vor pausiert).
- Cover-Hash: gleiches Cover → kein neues `setImage`.

## Hinweise

- `windows`-Crate nur unter `[target.'cfg(windows)'.dependencies]`, `zbus` nur unter Linux.
- SMTC-Aufrufe sind async (`IAsyncOperation`); mit `.get()` in `spawn_blocking` oder `.await` (Feature
  `windows-future`) auflösen.

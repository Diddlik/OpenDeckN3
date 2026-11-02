# 04 – Lautstärke pro App

Ein Drehregler pro Anwendung (Spiel, Discord, Browser), wie ein Mischpult.

- UUID `de.opendeckn3.mixer`, Ordner `plugins/mixer`, Win + Linux.
- Windows: WASAPI-Sitzungen über `windows` (Features `Win32_Media_Audio`, `Win32_System_Com`,
  `Win32_System_Threading`, `Win32_Foundation`): `IMMDeviceEnumerator` → Standard-Ausgabegerät →
  `IAudioSessionManager2` → Sitzungen → `ISimpleAudioVolume`. Prozessname über `GetProcessId` +
  `QueryFullProcessImageNameW`. COM in einem eigenen Thread (`CoInitializeEx`, MTA), Aufträge per Kanal.
- Linux: `pactl -f json list sink-inputs`, `pactl set-sink-input-volume <id> <n>%`, `pactl set-sink-input-mute <id> toggle`
  (läuft auch unter PipeWire mit `pipewire-pulse`). Zuordnung über `application.process.binary`.

## Aktionen

### `de.opendeckn3.mixer.app` – „App-Lautstärke“ (Encoder, Keypad)

| Feld | Typ | Bemerkung |
| --- | --- | --- |
| `app` | select (`source: "apps"`) | Prozessnamen der aktuellen Audio-Sitzungen, z. B. `Discord.exe`; Freitext erlaubt |
| `step` | number | Prozent je Raste, Standard 2, nur Encoder |

- Drehen: Lautstärke aller Sitzungen dieses Prozesses ändern (0–100 %). Drücken/Taste: stumm umschalten.
- Titel: `45 %` bzw. `stumm`; Zustand 1 = stumm. App nicht aktiv → Titel „–“.
- Abfrage alle 1 s, damit Änderungen aus dem Windows-Mixer sichtbar werden.

### `de.opendeckn3.mixer.focused` – „Aktive App“ (Encoder)

Wie oben, aber für die App im Vordergrund (Windows: `GetForegroundWindow` → Prozess; Linux: nicht unterstützt,
Titel „–“). Erst bauen, wenn `app` fertig ist.

## Tests

- Prozessnamen-Vergleich ohne Groß-/Kleinschreibung und mit/ohne `.exe`.
- Parser für `pactl`-JSON (Beispiel-Ausgabe als Testdatei).
- Lautstärke klemmen auf 0–100.

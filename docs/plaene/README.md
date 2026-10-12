# Pläne: neue Integrationen

Pipeline der geplanten Plugins. Abgearbeitet wird von oben nach unten, jeweils ein Plugin pro Commit.
Status: `offen` → `in Arbeit` → `fertig`.

| # | Plan | Plugin-UUID | Plattform | Status |
| --- | --- | --- | --- | --- |
| 01 | [HTTP-Anfrage / Webhook](01-http.md) | `de.opendeckn3.http` | Win, Linux | fertig |
| 02 | [Medien / Now Playing](02-media.md) | `de.opendeckn3.media` | Win, Linux | offen |
| 03 | [Systemmonitor](03-sysmon.md) | `de.opendeckn3.sysmon` | Win, Linux | offen |
| 04 | [Lautstärke pro App](04-mixer.md) | `de.opendeckn3.mixer` | Win, Linux | offen |
| 05 | [VoiceMeeter](05-voicemeeter.md) | `de.opendeckn3.voicemeeter` | Win | offen |
| 06 | [Twitch](06-twitch.md) | `de.opendeckn3.twitch` | Win, Linux | offen |
| 07 | [YouTube Live](07-youtube.md) | `de.opendeckn3.youtube` | Win, Linux | offen |
| 08 | [Licht: Elgato, Hue, WLED](08-lights.md) | `de.opendeckn3.lights` | Win, Linux | offen |
| 09 | [MQTT](09-mqtt.md) | `de.opendeckn3.mqtt` | Win, Linux | offen |
| 10 | [Microsoft Teams](10-teams.md) | `de.opendeckn3.teams` | Win | offen |
| 11 | [Timer / Pomodoro](11-timer.md) | `de.opendeckn3.timer` | Win, Linux | offen |
| 12 | [GitHub](12-github.md) | `de.opendeckn3.github` | Win, Linux | offen |
| 13 | [MIDI](13-midi.md) | `de.opendeckn3.midi` | Win, Linux | offen |
| 14 | [Spotify](14-spotify.md) | `de.opendeckn3.spotify` | Win, Linux | offen |

Verworfen:

- **Textbausteine:** gibt es schon als eingebaute Aktion „Text“ (`opendeckn3.builtin.text`).
- **Lightroom/Photoshop direkt:** keine stabile lokale API. Lightroom geht über MIDI + MIDI2LR (siehe Plan 13).
- **Zoom/Google Meet:** keine lokale API; Tastenkürzel decken es ab (eingebaute Aktion „Tastenkürzel“).

## Gemeinsame Checkliste für jedes Plugin

Vorlage ist ein bestehendes Rust-Plugin, z. B. `plugins/audioswitcher` (klein) oder `plugins/homeassistant` (HTTP).

1. `plugins/<name>/Cargo.toml`: Paket und Binary `opendeckn3-<name>`, `edition.workspace`, `license.workspace`,
   `publish = false`. Abhängigkeiten aus `[workspace.dependencies]` mit `.workspace = true`; HTTP immer
   `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls-native-roots"] }`.
2. `plugins/<name>/src/main.rs`: erste Zeile `#![cfg_attr(windows, windows_subsystem = "windows")]`, Verbindung über
   `n3_plugin_sdk::connect()`, Tasten über `Contexts`, Antworten auf Auswahllisten über `Host::reply_options`.
   Nutzertexte (Titel, Fehler, Logs an den Nutzer) auf Deutsch wie in den anderen Plugins; Code und Kommentare Englisch.
3. `plugins/<name>/de.opendeckn3.<name>.sdPlugin/manifest.json`: `Version` `0.1.0`, `CodePathWin`
   (`opendeckn3-<name>.exe`) und bei Linux-Unterstützung `CodePathLin`/`CodePathMac`, `OS`, `Actions` mit
   `SettingsSchema`, Zugangsdaten im `GlobalSettingsSchema` (`password`). Formular-Typen: `docs/PLUGIN_API.md`.
4. Root-`Cargo.toml`: Pfad in `members` **und** `default-members`.
5. Tastenbilder: Einträge in `tools/make-plugin-icons.cjs` ergänzen und erzeugen
   (`NODE_PATH=$(npm root -g) node tools/make-plugin-icons.cjs`, braucht Playwright). Nicht von Hand zeichnen.
6. `.github/workflows/windows.yml`: Paket bei `cargo build --release -p …` und Namen in der `foreach`-Liste ergänzen.
7. `plugins/registry.json`: Eintrag mit `asset` `de.opendeckn3.<name>-windows.streamDeckPlugin` und `homepage`.
8. Doku: `docs/PLUGIN_<NAME>.md` (Deutsch, Aufbau wie `PLUGIN_AUDIOSWITCHER.md`), Link in der Liste in
   `docs/PLUGIN_API.md`, Zeile in `docs/ENTWICKLUNG.md` (Verzeichnisübersicht), Punkt in der Feature-Liste von
   `README.md` **und** `README.de.md`. Neue Werkzeug-Abhängigkeiten in `AGENTS.md` nachtragen.
9. Tests für die reine Logik (Parsen, Formatieren, Zustandswechsel) im Plugin; keine Netzwerk-Tests gegen echte Dienste.
10. Prüfen: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`.
11. Status in dieser Tabelle auf `fertig` setzen, ein Commit pro Plugin.

## OAuth-Dienste

Twitch, YouTube und Spotify brauchen eine registrierte App. Wie beim Discord-Plugin trägt der Nutzer seine eigene
Client-ID in den globalen Einstellungen ein; die Doku erklärt die Registrierung Schritt für Schritt. Tokens werden in
den globalen Einstellungen gespeichert und automatisch erneuert. Es wird kein Secret mit dem Plugin ausgeliefert.

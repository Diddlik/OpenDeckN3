# Entwicklung

Technische Übersicht für alle, die OpenDeckN3 bauen, erweitern oder Plugins schreiben.
Die Projekt-Startseite ist das [README](../README.de.md).

## Dokumentation

| Dokument | Inhalt |
| --- | --- |
| [VORHABEN.md](VORHABEN.md) | **Definition des Vorhabens**: Ziele, Nicht-Ziele, Begriffe, Meilensteine |
| [ARCHITEKTUR.md](ARCHITEKTUR.md) | Komponenten, Datenfluss, Persistenz, Erweiterungspunkte |
| [GERAET_TREASLIN_N3.md](GERAET_TREASLIN_N3.md) | Hardware: Layout, HID-Codes, Bildformat, udev |
| [PLUGIN_API.md](PLUGIN_API.md) | Plugins schreiben: Manifest, Protokoll, Events |
| [UI_API.md](UI_API.md) | WebSocket-API für Oberflächen |
| [UI_DESIGN_BRIEF.md](UI_DESIGN_BRIEF.md) | **Design-Brief für Claude Design** |
| [images/](images/) | Screenshots fürs README – neu erzeugen siehe [tools/screenshots](../tools/screenshots/README.md) |

## Projektstruktur

```text
crates/
  n3-core/     Domänenmodell (Geräte, Eingaben, Profile)
  n3-driver/   TreasLin-N3-Treiber (via mirajazz), Hot-Plug, virtuelles Gerät
  n3-plugin/   Plugin-Host (Manifest, Prozesse, WebSocket-Protokoll)
  n3-daemon/   Dienst (Bibliothek + CLI `opendeckn3d`): Router, Profile, eingebaute Aktionen, UI-API
  n3-desktop/  Desktop-App „OpenDeckN3“ (Tauri): Fenster, Tray, Autostart, eingebetteter Dienst
plugins/examples/  Beispiel-Plugin „Zähler“ (Node, ohne Abhängigkeiten)
plugins/registry.json  Plugin-Katalog (GitHub-Releases), siehe docs/PLUGINS_VEROEFFENTLICHEN.md
plugins/discord/   Discord-Plugin (Rust, docs/PLUGIN_DISCORD.md); Tastenbilder: tools/make-discord-icons.cjs
plugins/obs/       OBS-Studio-Plugin (docs/PLUGIN_OBS.md)
plugins/homeassistant/  Home-Assistant-Plugin (docs/PLUGIN_HOMEASSISTANT.md); Tastenbilder beider: tools/make-plugin-icons.cjs
plugins/sdk/       Hilfsbibliothek für Plugins in Rust (Host-Verbindung, Tastenzustand, Einstellungen)
tools/smoke-test.mjs  End-to-End-Test gegen den laufenden Dienst
udev/          Linux-Regeln für Gerätezugriff
assets/        App-Icon (erzeugt mit tools/make-icon.py)
ui/            Oberfläche (Desktop-App-Fenster bzw. Browser) + Claude-Design-Prototyp
```

## Windows: Desktop-App & Releases

OpenDeckN3 ist unter Windows eine Desktop-App (Tauri): eigenes Fenster, Symbol im Infobereich (Tray), kein Browser,
kein Konsolenfenster. Der Dienst läuft in der App mit.

1. Neueste Version unter [Releases](https://github.com/Diddlik/OpenDeckN3/releases) laden – oder den neuesten Lauf unter
   **Actions → Windows-Installer** öffnen und bei **Artifacts** `OpenDeckN3-…-windows-x64` herunterladen.
2. `OpenDeckN3-…-setup.exe` ausführen (keine Admin-Rechte nötig; SmartScreen-Warnung, da unsigniert:
   „Weitere Informationen“ → „Trotzdem ausführen“). Alternativ das portable ZIP entpacken und `OpenDeckN3.exe` starten.
3. **Fenster schließen** = App läuft im Tray weiter. **Beenden** über Rechtsklick aufs Tray-Symbol.
   Autostart lässt sich in den Einstellungen einschalten (startet dann unsichtbar im Tray).

**Release erstellen:** *Actions → Windows-Installer → Run workflow* mit `release_tag` (z. B. `v0.1.0-alpha.2`) starten –
GitHub legt Tag und Release mit Installer und ZIP an. Tags mit Bindestrich (`-alpha`, `-beta`) werden als Pre-release markiert.
Ein lokal gepushtes Tag `v*` funktioniert ebenso. Jedes Release enthält `SHA256SUMS.txt` – daran prüft der Auto-Updater der App
den Installer, bevor er ihn startet. Fertige Versionen: [Releases](https://github.com/Diddlik/OpenDeckN3/releases).

Hinweise: Profile und Log (`opendeckn3.log`) liegen in `%APPDATA%\opendeckn3`. Das Beispiel-Plugin braucht Node.js ≥ 22.
Die Hersteller-Software des N3 sollte nicht gleichzeitig laufen. Ein zweiter Start holt nur das vorhandene Fenster nach vorn.
Benötigt die WebView2-Laufzeit (in Windows 10/11 enthalten, der Installer lädt sie sonst nach).

Desktop-App selbst bauen: `cd crates/n3-desktop && npx @tauri-apps/cli@2 build` (Installer) oder
`cargo run -p n3-desktop` (Entwicklung; unter Linux werden `libwebkit2gtk-4.1-dev` und `libayatana-appindicator3-dev` benötigt).

## Schnellstart

Voraussetzungen: Rust ≥ 1.88, für Plugins/Tests Node.js ≥ 22.

```sh
# Bauen und testen
cargo build
cargo test

# Linux: Zugriff auf das Gerät erlauben (einmalig)
sudo cp udev/40-opendeckn3.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger

# Dienst mit echtem Gerät starten
cargo run -p n3-daemon

# Desktop-App starten (Fenster statt Browser)
cargo run -p n3-desktop

# …oder nur den Dienst, ohne Hardware mit virtuellem N3 und Beispiel-Plugin
cargo run -p n3-daemon -- --virtual --no-hardware --plugins-dir plugins/examples

# Oberfläche im Browser öffnen
#   http://127.0.0.1:57132/

# In zweitem Terminal: End-to-End-Test über die UI-API
node tools/smoke-test.mjs
```

Optionen: `opendeckn3d --help` (`--config-dir`, `--plugins-dir` (zusätzlich zu `<config>/plugins`), `--open`, `--plugin-port`, `--api-port`, `--ui-port`, `--no-ui`, `--allow-origin`, `--virtual`, `--no-hardware`).
Log-Level über `RUST_LOG`, z. B. `RUST_LOG=debug` oder `RUST_LOG=n3_driver=trace`.

## Lizenz

GPL-3.0-or-later, siehe [LICENSE](../LICENSE).

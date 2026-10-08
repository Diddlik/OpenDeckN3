# OpenDeckN3

Quelloffene Steuer-Software für Stream-Controller – zuerst für den **TreasLin N3** (USB `5548:1001`) –
mit Plugin-System (Stream-Deck-SDK-kompatibel), Profilen und einer API für eine moderne Oberfläche.

> **Status:** Grundgerüst (Meilenstein M0) plus Weboberfläche nach dem Claude-Design-Entwurf.
> Dienst, N3-Treiber, Plugin-Host, Profile, UI-API und Oberfläche funktionieren (getestet mit dem virtuellen Gerät).

Inspiriert von und basierend auf Erkenntnissen aus
[nekename/OpenDeck](https://github.com/nekename/OpenDeck) und
[4ndv/opendeck-akp03](https://github.com/4ndv/opendeck-akp03).

## Dokumentation

| Dokument | Inhalt |
| --- | --- |
| [docs/VORHABEN.md](docs/VORHABEN.md) | **Definition des Vorhabens**: Ziele, Nicht-Ziele, Begriffe, Meilensteine |
| [docs/ARCHITEKTUR.md](docs/ARCHITEKTUR.md) | Komponenten, Datenfluss, Persistenz, Erweiterungspunkte |
| [docs/GERAET_TREASLIN_N3.md](docs/GERAET_TREASLIN_N3.md) | Hardware: Layout, HID-Codes, Bildformat, udev |
| [docs/PLUGIN_API.md](docs/PLUGIN_API.md) | Plugins schreiben: Manifest, Protokoll, Events |
| [docs/UI_API.md](docs/UI_API.md) | WebSocket-API für Oberflächen |
| [docs/UI_DESIGN_BRIEF.md](docs/UI_DESIGN_BRIEF.md) | **Design-Brief für Claude Design** |

## Projektstruktur

```text
crates/
  n3-core/     Domänenmodell (Geräte, Eingaben, Profile)
  n3-driver/   TreasLin-N3-Treiber (via mirajazz), Hot-Plug, virtuelles Gerät
  n3-plugin/   Plugin-Host (Manifest, Prozesse, WebSocket-Protokoll)
  n3-daemon/   Dienst `opendeckn3d`: Router, Profile, eingebaute Aktionen, UI-API
plugins/examples/  Beispiel-Plugin „Zähler“ (Node, ohne Abhängigkeiten)
tools/smoke-test.mjs  End-to-End-Test gegen den laufenden Dienst
udev/          Linux-Regeln für Gerätezugriff
ui/            Weboberfläche (vom Dienst ausgeliefert) + Claude-Design-Prototyp
```

## Schnellstart

Voraussetzungen: Rust ≥ 1.87, für Plugins/Tests Node.js ≥ 22.

```sh
# Bauen und testen
cargo build
cargo test

# Linux: Zugriff auf das Gerät erlauben (einmalig)
sudo cp udev/40-opendeckn3.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger

# Dienst mit echtem Gerät starten
cargo run -p n3-daemon

# …oder ohne Hardware mit virtuellem N3 und Beispiel-Plugin
cargo run -p n3-daemon -- --virtual --no-hardware --plugins-dir plugins/examples

# Oberfläche im Browser öffnen
#   http://127.0.0.1:57132/

# In zweitem Terminal: End-to-End-Test über die UI-API
node tools/smoke-test.mjs
```

Optionen: `opendeckn3d --help` (`--config-dir`, `--plugins-dir`, `--plugin-port`, `--api-port`, `--ui-port`, `--no-ui`, `--allow-origin`, `--virtual`, `--no-hardware`).
Log-Level über `RUST_LOG`, z. B. `RUST_LOG=debug` oder `RUST_LOG=n3_driver=trace`.

## Lizenz

GPL-3.0-or-later, siehe [LICENSE](LICENSE).

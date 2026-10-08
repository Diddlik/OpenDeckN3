# OpenDeckN3 – Definition des Vorhabens

> Stand: Oktober 2026 · Status: **Grundgerüst (M0)**

## 1. Worum geht es?

OpenDeckN3 ist eine **eigenständige, quelloffene Steuer-Software für Stream-Controller**,
beginnend mit dem **TreasLin N3** (USB `5548:1001`). Sie ersetzt die Hersteller-Software,
läuft unter Linux, Windows und macOS und ist über **Plugins** erweiterbar.

Statt – wie bisher – OpenDeck plus ein Fremd-Plugin (`opendeck-akp03`) zu kombinieren, bündeln
wir Geräteansteuerung, Profilverwaltung, Plugin-System und Oberfläche in einem Projekt, das
von Anfang an auf die N3-Familie (6 Display-Tasten, 3 Tasten, 3 Drehregler) zugeschnitten ist.

### Ausgangspunkte

| Projekt | Was wir übernehmen | Was wir anders machen |
| --- | --- | --- |
| [4ndv/opendeck-akp03](https://github.com/4ndv/opendeck-akp03) | HID-Protokoll des N3 über die Bibliothek `mirajazz`, Tasten-/Drehregler-Codes, Bildformat (64×64 JPEG, 90° gedreht), udev-Regeln | Kein OpenDeck-Plugin, sondern direkt eingebauter Treiber |
| [nekename/OpenDeck](https://github.com/nekename/OpenDeck) | Plugin-Modell (Stream-Deck-SDK-kompatibles WebSocket-Protokoll, `manifest.json`), Profil-/Kontext-Konzept | Schlanker Rust-Kern ohne Tauri-Abhängigkeit, getrennte UI über eine dokumentierte API, Fokus auf N3-Layout inkl. Drehreglern |

## 2. Ziele

1. **Hardware-Unterstützung TreasLin N3** – Erkennung per Hot-Plug, Tasten, Drehregler (drehen + drücken),
   Bilder auf den 6 Display-Tasten, Helligkeit, Keep-Alive.
2. **Plugin-System** – Plugins sind eigene Prozesse in beliebiger Sprache und sprechen ein
   Stream-Deck-SDK-kompatibles Protokoll. Ziel: viele bestehende Stream-Deck-/OpenDeck-Plugins
   laufen ohne Anpassung.
3. **Profile** – mehrere Profile pro Gerät, schnelles Umschalten (auch per Taste), persistente
   Einstellungen pro Aktion.
4. **Moderne Oberfläche** – visuell ansprechende Desktop-UI (Entwurf mit *Claude Design*), die
   ausschließlich über die dokumentierte UI-API mit dem Dienst spricht.
5. **Entwicklung ohne Hardware** – ein virtuelles N3 erlaubt UI- und Plugin-Entwicklung ohne Gerät.
6. **Erweiterbarkeit auf weitere Geräte** – weitere Mirabox-/Ajazz-Modelle sollen sich durch einen
   Eintrag in der Modelltabelle ergänzen lassen.

## 3. Nicht-Ziele (vorerst)

- Kein Plugin-Marktplatz und keine Cloud-Synchronisierung.
- Keine Unterstützung für Elgato-Hardware (dafür gibt es OpenDeck).
- Kein Ausführen von Windows-Plugins per Wine (evtl. später).
- Keine Property-Inspector-HTML-Seiten der Plugins im MVP (Einstellungen zunächst als JSON/Formular).

## 4. Zielgruppe

- Linux-Nutzer:innen mit einem günstigen N3-Controller, für die es keine Hersteller-Software gibt.
- Streamer:innen, Entwickler:innen und Power-User, die Makros, Medien- und Smart-Home-Steuerung wollen.
- Plugin-Autor:innen, die mit wenig Aufwand eigene Aktionen bauen wollen.

## 5. Kernbegriffe

| Begriff | Bedeutung |
| --- | --- |
| **Gerät** | Ein angeschlossener Controller, Id `n3-<Seriennummer>` (virtuell: `virtual-n3`). |
| **Slot** | Eine Taste (`Keypad`, Position 0–8), das Drücken eines Drehreglers (`Keypad`, Position 9–11) oder das Drehen eines Drehreglers (`Encoder`, Position 0–2). |
| **Modus** | Die Oberfläche belegt entweder **Drehregler** (nur Drehen, nur drehfähige Aktionen) oder **Tasten** (9 Tasten + 3 Regler als Taste, alle Tasten-Aktionen). |
| **Aktion** | Funktion aus einem Plugin, z. B. „Lautstärke“, „Profil wechseln“. |
| **Instanz** | Eine Aktion, die auf einem Slot liegt – mit eigenen Einstellungen (`settings`) und Zustand (`state`). |
| **Profil** | Satz von Instanzen für ein Gerät. Genau ein Profil ist pro Gerät aktiv. |
| **Plugin** | Externer Prozess mit `manifest.json`, der Aktionen bereitstellt. |
| **Kontext** | Eindeutige Id einer Instanz im Plugin-Protokoll (`<gerät>\|<profil>\|key\|<pos>`). |
| **Dienst / Daemon** | `opendeckn3d` – läuft im Hintergrund, besitzt Geräte, Profile und Plugins. |
| **UI** | Grafische Oberfläche, spricht nur über die UI-API mit dem Dienst. |

## 6. Funktionsumfang nach Meilensteinen

### M0 – Grundgerüst ✅ (dieser Stand)
- Rust-Workspace mit Crates `n3-core`, `n3-driver`, `n3-plugin`, `n3-daemon`.
- TreasLin-N3-Treiber (Hot-Plug, Eingaben, Bilder, Helligkeit, Keep-Alive).
- Virtuelles N3 (`--virtual`).
- Plugin-Host: Discovery, Prozessstart (Binary, Node, Python), WebSocket-Protokoll (Kernereignisse).
- Profile & Einstellungen als JSON im Konfigurationsverzeichnis.
- Eingebaute Aktionen: Helligkeit, Profil wechseln.
- UI-WebSocket-API inkl. Live-Ereignissen und Eingabe-Simulation.
- Beispiel-Plugin „Zähler“ (Node, ohne Abhängigkeiten) und End-to-End-Smoke-Test.

### M0+ – Weboberfläche ✅
- Oberfläche nach dem Claude-Design-Entwurf (`ui/index.html`), vom Dienst unter `http://127.0.0.1:57132/` ausgeliefert:
  Editor mit Drag & Drop, Inspektor, Profile, Helligkeit, Plugins, Einstellungen, Onboarding, Offline-/Leer-Zustände.
- Origin-Prüfung der UI-API, `startVirtualDevice`.

### M1 – Benutzbar
- ✅ Desktop-App (Tauri) mit eingebettetem Dienst: Fenster, Tray, Autostart, Single-Instance, NSIS-Installer für Windows.
- ✅ Auto-Update: prüft GitHub-Releases, fragt nach, lädt den Installer, verifiziert ihn per SHA-256 und startet neu.
- ✅ Titel auf den Tasten-Displays (Geist-Schrift, automatisch verkleinert/gekürzt).
- Plugin-Installation aus `.zip`/`.streamDeckPlugin`, Plugin-Neustart bei Absturz.
- Tray-Icon, Autostart, Bildschirmschoner/Sleep nach Inaktivität.
- ✅ Eingebaute Aktionen wie im OpenDeck-Starterpaket: Tastenkürzel, Lautstärke, Medien, Programm öffnen, Website, Befehl,
  Text eingeben – mit Einstellungsformularen in der UI.
- Multi-Aktion, Ordner/Seiten.

### M2 – Komfort
- Property Inspector (HTML-Einstellungsseiten der Plugins) in der UI.
- Automatischer Profilwechsel je aktivem Programm.
- Import/Export von Profilen, Backup.
- Animierte Bilder (GIF), SVG-Icons.
- Weitere Geräte der N3/AKP03-Familie über die Modelltabelle.

### M3 – Ökosystem
- Plugin-SDK-Pakete (TypeScript, Python, Rust) mit Typen für das Protokoll.
- Plugin-Katalog (lokal/aus GitHub-Releases).
- Optional: Wine-Unterstützung für Windows-only-Plugins.

## 7. Qualitätsanforderungen

- **Robustheit:** Abziehen/Anstecken des Geräts, Plugin-Abstürze und kaputte Bilder dürfen den Dienst nie beenden.
- **Latenz:** Tastendruck → Plugin-Ereignis < 20 ms; Bild-Update sichtbar < 100 ms.
- **Ressourcen:** Dienst im Leerlauf < 30 MB RAM, ~0 % CPU.
- **Sicherheit:** Alle Ports binden nur an `127.0.0.1`; Plugins dürfen nur ihre eigenen Kontexte ändern;
  Profil-/Geräte-Ids werden gegen Pfad-Traversal geprüft; `openUrl` nur für http(s).
- **Testbarkeit:** Jede Logik ohne Hardware testbar (virtuelles Gerät, Smoke-Test).

## 8. Lizenz

GPL-3.0-or-later – wie die beiden Ausgangsprojekte, deren Ideen und Teile wir übernehmen.
Die Bibliothek `mirajazz` steht unter MPL-2.0 und ist damit kompatibel.

## 9. Weiterführende Dokumente

- [Architektur](ARCHITEKTUR.md)
- [Gerät: TreasLin N3](GERAET_TREASLIN_N3.md)
- [Plugin-API](PLUGIN_API.md)
- [UI-API](UI_API.md)
- [Design-Brief für die Oberfläche (Claude Design)](UI_DESIGN_BRIEF.md)

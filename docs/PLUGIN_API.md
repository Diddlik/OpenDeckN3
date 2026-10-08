# Plugin-API

OpenDeckN3-Plugins sind **eigenständige Prozesse**, die sich per WebSocket mit dem Dienst verbinden.
Das Protokoll ist eine Teilmenge des **Elgato Stream Deck SDK** (wie bei OpenDeck/OpenAction) –
bestehende Plugins funktionieren daher oft unverändert.

Ein vollständiges, abhängigkeitsfreies Beispiel liegt unter
[`plugins/examples/de.opendeckn3.counter.sdPlugin`](../plugins/examples/de.opendeckn3.counter.sdPlugin).

## Aufbau eines Plugins

```text
de.example.myplugin.sdPlugin/
├── manifest.json
├── plugin.js            # oder .py, oder ein Binary
└── imgs/action.png      # Icons (optional, ohne Endung im Manifest referenzierbar)
```

### `manifest.json`

```json
{
  "UUID": "de.example.myplugin",
  "Name": "Mein Plugin",
  "Author": "Ich",
  "Version": "1.0.0",
  "Category": "Werkzeuge",
  "CodePath": "plugin.js",
  "CodePathLin": "bin/linux",
  "CodePathWin": "bin/win.exe",
  "CodePathMac": "bin/mac",
  "Actions": [
    {
      "UUID": "de.example.myplugin.toggle",
      "Name": "Umschalten",
      "Icon": "imgs/action",
      "Tooltip": "Schaltet etwas um",
      "Controllers": ["Keypad", "Encoder"],
      "States": [{ "Image": "imgs/off" }, { "Image": "imgs/on" }]
    }
  ]
}
```

- `UUID` fehlt → Verzeichnisname ohne `.sdPlugin` wird verwendet. `opendeckn3.builtin` ist reserviert.
- `CodePath*`: plattformspezifischer Pfad hat Vorrang. Endung `.js/.mjs/.cjs` → Start mit `node`,
  `.py` → `python3`, sonst direkt ausführbar.
- `Controllers`: Standard `["Keypad"]`.

### Einstellungsformulare (OpenDeckN3-Erweiterung)

Property-Inspector-Seiten (HTML) zeigt OpenDeckN3 nicht an. Stattdessen beschreibt ein Plugin seine Einstellungen im
Manifest; die App baut daraus Formulare:

- **`SettingsSchema`** an einer Aktion → Felder im Inspektor der belegten Taste. Werte landen in `settings`
  (Plugin erhält `didReceiveSettings`). Felder mit `default` werden beim Belegen vorausgefüllt.
- **`GlobalSettingsSchema`** am Plugin → Abschnitt „Einstellungen“ auf der Plugins-Seite. Werte landen in den globalen
  Einstellungen (Plugin erhält `didReceiveGlobalSettings`). Ideal für Zugangsdaten.

```json
"SettingsSchema": [
  { "type": "select", "key": "mode", "label": "Funktion", "default": "toggle",
    "options": [["toggle", "Umschalten"], ["on", "Ein"], ["off", "Aus"]] },
  { "type": "number", "key": "step", "label": "Schritt", "default": 5, "min": 1, "max": 50,
    "controllers": ["Encoder"], "help": "Pro Raste" }
]
```

| `type` | Darstellung |
| --- | --- |
| `text`, `textarea`, `number`, `select`, `file`, `shortcut`, `profile` | Eingabefelder wie bei den eingebauten Aktionen |
| `password` | verdecktes Feld (nur `GlobalSettingsSchema`) |
| `info` | zeigt den Wert von `key` nur an, z. B. einen Status, den das Plugin selbst per `setGlobalSettings` schreibt |
| `button` | Knopf; setzt `key` auf einen neuen Zeitstempel – das Plugin reagiert auf die Änderung |
| `link` | Knopf, der `url` im Browser öffnet |

Gemeinsame Felder: `key`, `label`, `help`, `placeholder`, `default`, `controllers` (nur für `Keypad`/`Encoder` zeigen),
`showIf` (`{ "authMode": "manual" }` – Feld nur zeigen, wenn andere Felder diese Werte haben). Knöpfe mit
`"primary": true` werden hervorgehoben und erscheinen bei getrennter Verbindung auch im Inspektor.

**Auswahllisten aus dem Plugin:** Ein `select` mit `source` statt `options` bekommt seine Einträge zur Laufzeit vom
Plugin, z. B. Discord-Server und -Kanäle. `dependsOn` nennt Felder, deren Werte mitgeschickt werden (Kanal hängt vom
Server ab). Die App sendet dem Plugin

```json
{ "event": "sendToPlugin", "context": "<plugin-uuid>", "payload": { "request": "voiceChannels", "guild": "123", "requestId": 7 } }
```

und erwartet als Antwort `sendToPropertyInspector` mit derselben `requestId` und `options` (oder `error`):

```json
{ "event": "sendToPropertyInspector", "context": "<plugin-uuid>", "payload": { "requestId": 7, "options": [["456", "🔊 Lounge"]] } }
```
Ändert ein Plugin seine globalen Einstellungen selbst, aktualisiert die App das Formular (`globalSettingsChanged`).
Vollständige Beispiele: [Discord](PLUGIN_DISCORD.md), [OBS Studio](PLUGIN_OBS.md),
[Home Assistant](PLUGIN_HOMEASSISTANT.md). Für Plugins in Rust nimmt [`plugins/sdk`](../plugins/sdk) die
Verbindung zum Host, die Tasten-Verwaltung und die Antworten auf Auswahllisten ab.

## Start & Registrierung

Der Dienst startet das Plugin mit:

```text
<code> -port 57130 -pluginUUID <uuid> -registerEvent registerPlugin -info '<json>'
```

Das Plugin verbindet sich mit `ws://127.0.0.1:<port>` und sendet **als erste Nachricht**:

```json
{ "event": "registerPlugin", "uuid": "<uuid>" }
```

Danach erhält es `deviceDidConnect` für alle Geräte und `willAppear` für alle seine Instanzen
im aktiven Profil.

## Kontext

Jede Instanz hat einen `context`-String (`<gerät>|<profil>|key|<pos>` bzw. `…|enc|<pos>`).
Plugins sollen ihn als **opak** behandeln und nur zurückschicken. Der Dienst akzeptiert
Änderungen nur für Kontexte, die dem sendenden Plugin gehören und im aktiven Profil liegen.

## Ereignisse: Dienst → Plugin

Gemeinsame Felder: `event`, `action`, `context`, `device`, `payload`.
`payload` enthält immer `settings`, `coordinates {column,row}`, `controller` (`Keypad`/`Encoder`),
`state`, `isInMultiAction`.

| Event | Wann | Zusätzliche Payload-Felder |
| --- | --- | --- |
| `deviceDidConnect` | Gerät verbunden / nach Registrierung | `deviceInfo {name,type,size}` (kein `action/context`) |
| `deviceDidDisconnect` | Gerät getrennt | – |
| `willAppear` | Instanz wird sichtbar (Profil aktiv / neu platziert) | – |
| `willDisappear` | Instanz verschwindet | – |
| `keyDown` / `keyUp` | Taste gedrückt / losgelassen | – |
| `dialDown` / `dialUp` | Drehregler gedrückt / losgelassen (nur wenn der Regler keine eigene Drück-Belegung hat – sonst bekommt diese `keyDown`/`keyUp` als `Keypad`, Position 9–11) | – |
| `dialRotate` | Drehregler gedreht | `ticks` (±n), `pressed` |
| `didReceiveSettings` | Antwort auf `getSettings` oder Änderung durch die UI | – |
| `didReceiveGlobalSettings` | Antwort auf `getGlobalSettings` | `settings` |

## Ereignisse: Plugin → Dienst

| Event | Felder | Wirkung |
| --- | --- | --- |
| `setSettings` | `context`, `payload` (Objekt) | Einstellungen der Instanz speichern |
| `getSettings` | `context` | löst `didReceiveSettings` aus |
| `setGlobalSettings` | `context` = Plugin-UUID, `payload` | globale Einstellungen speichern |
| `getGlobalSettings` | `context` = Plugin-UUID | löst `didReceiveGlobalSettings` aus |
| `setImage` | `context`, `payload.image` (Data-URL PNG/JPEG/BMP oder `null`) | Tastenbild setzen / zurücksetzen |
| `setTitle` | `context`, `payload.title` | Titel setzen – wird unten auf das Tastenbild gezeichnet (ein vom Nutzer gesetzter Titel hat Vorrang) |
| `setState` | `context`, `payload.state` | Zustand wechseln (Bild aus `States`) |
| `showOk` / `showAlert` | `context` | Rückmeldung (aktuell nur an die UI) |
| `logMessage` | `payload.message` | Eintrag im Dienst-Log |
| `openUrl` | `payload.url` | http(s)-URL im Standardbrowser öffnen |

Nicht unterstützte Events werden ignoriert (Debug-Log).

## Verteilen & installieren

Ein Plugin wird als ZIP des `<uuid>.sdPlugin`-Ordners weitergegeben (Endung `.streamDeckPlugin` oder `.zip`).
Installiert wird es in der App unter **Plugins → Aus Datei …** (oder Datei auf die Seite ziehen) bzw. aus einem
GitHub-Release über **Plugins → Katalog**. Wie man ein Plugin auf GitHub veröffentlicht und in den Katalog bringt:
[PLUGINS_VEROEFFENTLICHEN.md](PLUGINS_VEROEFFENTLICHEN.md).

Bei einem Update beendet der Dienst den alten Prozess, ersetzt den Ordner und startet die neue Version; danach
bekommt das Plugin wie gewohnt `willAppear` für seine Belegungen.

## Noch nicht unterstützt

`sendToPropertyInspector`, `sendToPlugin`, `setFeedback`, `setFeedbackLayout`, `switchToProfile`,
`setTriggerDescription`, Property-Inspector-Seiten, `ApplicationsToMonitor`.

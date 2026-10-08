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
| `dialDown` / `dialUp` | Drehregler gedrückt / losgelassen | – |
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

## Noch nicht unterstützt

`sendToPropertyInspector`, `sendToPlugin`, `setFeedback`, `setFeedbackLayout`, `switchToProfile`,
`setTriggerDescription`, Property-Inspector-Seiten, `ApplicationsToMonitor`.

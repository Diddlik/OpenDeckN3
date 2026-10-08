# UI-API

Die Oberfläche spricht **ausschließlich** über diese WebSocket-API mit dem Dienst.
Dadurch kann die UI unabhängig gestaltet und ausgetauscht werden (Tauri-App, Web-UI, CLI …).

- Adresse: `ws://127.0.0.1:57131` (änderbar mit `--api-port`)
- Format: JSON-Textnachrichten
- Zum Entwickeln ohne Hardware: `opendeckn3d --virtual --no-hardware`
- Die Desktop-App zeigt die Oberfläche im eigenen Fenster; der CLI-Dienst liefert sie zusätzlich unter
  `http://127.0.0.1:57132/` aus (`--ui-port`, abschaltbar mit `--no-ui`).

### Zugriffsschutz (Origin)

Browser senden beim Verbindungsaufbau immer einen `Origin`-Header. Der Dienst akzeptiert nur:

- Verbindungen **ohne** `Origin` (native Programme, Node-Skripte, Tauri-Backend),
- die eigene Weboberfläche (`http://127.0.0.1:<ui-port>`, `http://localhost:<ui-port>`),
- `tauri://localhost` / `http(s)://tauri.localhost` (Desktop-App `crates/n3-desktop`),
- zusätzliche Origins per `--allow-origin <origin>` (z. B. ein Vite-Dev-Server).

Alle anderen Origins erhalten HTTP 403 – so kann keine fremde Webseite das Gerät fernsteuern.

## Nachrichtenformat

**Anfrage** (Client → Dienst): `command` + Parameter, optional `id` zur Zuordnung der Antwort.

```json
{ "id": 1, "command": "getState" }
```

**Antwort** (Dienst → Client):

```json
{ "id": 1, "ok": true, "result": { } }
{ "id": 1, "ok": false, "error": "unknown device n3-xyz" }
```

**Event** (Dienst → alle Clients, ungefragt): hat ein Feld `event`, kein `id`.

```json
{ "event": "keyImage", "device": "virtual-n3", "key": 0, "image": "data:image/png;base64,..." }
```

## Datentypen

```ts
type Controller = "Keypad" | "Encoder";

interface DeviceSnapshot {
  info: {
    id: string;              // "n3-355499441494" | "virtual-n3"
    name: string;            // "TreasLin N3"
    vendorId: number; productId: number;
    serial: string; firmware: string | null;
    virtualDevice: boolean;
    layout: {
      rows: 3; columns: 3;
      keys: 9;               // Keypad 0..8, row-major
      displayKeys: 6;        // Keypad 0..5 haben ein Display
      encoders: 3;           // Encoder 0..2
      keyImageSize: [64, 64];
    };
  };
  brightness: number;        // 0..100
  activeProfile: string;
  profiles: string[];        // alle Profil-Ids des Geräts
  profile: Profile;          // das aktive Profil
  previews: Record<string, string>; // Taste → PNG-Data-URL (aktuelles Bild auf dem Gerät)
  titles: Record<string, string>;   // Taste → Titel, von Plugins zur Laufzeit gesetzt
}

interface Profile {
  id: string; name: string;
  keys: Record<string, ActionInstance>;      // "0".."8"
  encoders: Record<string, ActionInstance>;  // "0".."2"
}

interface ActionInstance {
  plugin: string;            // Plugin-UUID oder "opendeckn3.builtin"
  action: string;            // Aktions-UUID
  settings: object;
  state: number;
  title: string | null;      // vom Nutzer gesetzt
  image: string | null;      // vom Nutzer gesetzt (Data-URL)
}

interface CatalogPlugin {
  uuid: string; name: string; author: string; version: string;
  category: string;
  connected: boolean;        // Plugin-Prozess läuft und ist registriert
  actions: {
    uuid: string; name: string; tooltip: string;
    controllers: Controller[];
    icon: string | null;     // PNG-Data-URL
  }[];
}

type InputEvent =
  | { type: "keyDown"; key: number } | { type: "keyUp"; key: number }
  | { type: "encoderDown"; encoder: number } | { type: "encoderUp"; encoder: number }
  | { type: "encoderTwist"; encoder: number; ticks: number };
```

## Kommandos

| Kommando | Parameter | Ergebnis |
| --- | --- | --- |
| `getState` | – | `{ devices: DeviceSnapshot[], catalog: CatalogPlugin[] }` |
| `getCatalog` | – | `CatalogPlugin[]` |
| `switchProfile` | `device`, `profile` | `null` – legt das Profil an, falls es fehlt |
| `deleteProfile` | `device`, `profile` | `null` – aktives Profil kann nicht gelöscht werden |
| `setAction` | `device`, `controller`, `position`, `plugin`, `action`, `settings?` | `null` – ersetzt eine vorhandene Belegung |
| `clearAction` | `device`, `controller`, `position` | `null` |
| `setActionSettings` | `device`, `controller`, `position`, `settings` | `null` – Plugin erhält `didReceiveSettings` |
| `setActionAppearance` | `device`, `controller`, `position`, `title?`, `image?` | `null` – `image: null` = Standardbild |
| `setBrightness` | `device`, `value` (0–100) | `null` |
| `simulateInput` | `device`, `input: InputEvent` | `null` – wie echte Eingabe (ideal für das virtuelle Gerät) |
| `startVirtualDevice` | – | `{ device: "virtual-n3" }` – startet das virtuelle N3, falls es noch nicht läuft |

Profil-Ids dürfen nur `A-Z a-z 0-9 - _ .` enthalten und nicht mit `.` beginnen.

## Events

| Event | Felder | Hinweis für die UI |
| --- | --- | --- |
| `deviceConnected` | `device: DeviceSnapshot` | Gerät erscheint |
| `deviceDisconnected` | `device: string` | Gerät verschwindet |
| `profileChanged` | `device: DeviceSnapshot` | aktives Profil oder Profilliste geändert |
| `slotChanged` | `device: string` | Belegung/Einstellungen geändert → `getState` neu laden |
| `keyImage` | `device`, `key`, `image: string \| null` | Vorschau einer Taste aktualisieren |
| `keyTitle` | `device`, `controller`, `position`, `title` | Titel aktualisieren |
| `brightnessChanged` | `device`, `value` | Regler aktualisieren |
| `input` | `device`, `input: InputEvent` | Taste/Drehregler in der UI kurz aufleuchten lassen |
| `feedback` | `device`, `controller`, `position` | Plugin meldet OK/Fehler (Animation) |
| `pluginStatus` | `plugin`, `connected` | Plugin-Status im Katalog |
| `lagged` | `missed` | Client war zu langsam → `getState` neu laden |

## Beispiel-Sitzung

```js
const ws = new WebSocket("ws://127.0.0.1:57131");
ws.onopen = () => {
  ws.send(JSON.stringify({ id: 1, command: "getState" }));
  ws.send(JSON.stringify({
    id: 2, command: "setAction", device: "virtual-n3",
    controller: "Encoder", position: 1,
    plugin: "opendeckn3.builtin", action: "opendeckn3.builtin.brightness",
  }));
  ws.send(JSON.stringify({
    id: 3, command: "simulateInput", device: "virtual-n3",
    input: { type: "encoderTwist", encoder: 1, ticks: 2 },
  }));
};
ws.onmessage = (m) => console.log(JSON.parse(m.data));
```

Ein kompletter, automatisierter Ablauf steht in [`tools/smoke-test.mjs`](../tools/smoke-test.mjs).

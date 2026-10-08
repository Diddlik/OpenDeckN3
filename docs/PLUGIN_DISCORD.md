# Discord-Plugin

Steuert die **Discord-Desktop-App** vom TreasLin N3 aus – über Discords lokale RPC-Schnittstelle (IPC: Named Pipe
`\\.\pipe\discord-ipc-0` unter Windows, Unix-Socket `discord-ipc-0` unter Linux/macOS). Es wird kein Discord-Passwort und
kein Benutzer-Token ausgelesen; der Zugriff wird über Discords eigenes Bestätigungsfenster erteilt.

Quellcode: [`plugins/discord`](../plugins/discord) (Rust, ein einzelnes Programm ohne Laufzeit-Abhängigkeiten).

## Aktionen

| Aktion | Taste | Drehregler | Anzeige auf der Taste |
| --- | --- | --- | --- |
| Mikrofon stummschalten | umschalten / ein / aus | – | rot, wenn stumm |
| Kopfhörer stummschalten | umschalten / ein / aus | – | rot, wenn taub geschaltet |
| Sprachkanal | betreten / verlassen (Kanal-ID) | – | grün, wenn im Kanal |
| Textkanal | Kanal öffnen (Kanal-ID) | – | – |
| Benachrichtigung | Zähler zurücksetzen | – | Anzahl neuer Benachrichtigungen |
| Benutzer-Lautstärkeregelung | lauter / leiser / stumm (Benutzer-ID) | drehen = 0–200 %, drücken = stumm | Lautstärke in % |
| Lautstärkeregelung | lauter / leiser / stumm (Eingabe oder Ausgabe) | drehen = Lautstärke, drücken = stumm | Lautstärke in % |
| Sprachboard | Sound abspielen (Name oder ID) | – | – |
| Audiogerät einstellen | Eingabe-/Ausgabegerät wählen (Name oder Teil davon) | – | grün, wenn aktiv |
| Kamera ein-/ausschalten | umschalten | – | grün, wenn an |
| Bildschirmfreigabe ein-/ausschalten | umschalten | – | grün, wenn aktiv |
| Drücken zum Sprechen | solange gedrückt: Mikro an, danach wieder stumm | – | grün, solange gedrückt |
| Sprachaktivierung umschalten | Sprachaktivierung ↔ Push-to-Talk | – | Tastatur-Symbol bei Push-to-Talk |

Kanal- und Benutzer-IDs: in Discord **Einstellungen → Erweitert → Entwicklermodus** einschalten, dann Rechtsklick auf
den Kanal bzw. die Person → „ID kopieren“.

## Einrichtung (einmalig, ca. 3 Minuten)

Discord lässt RPC nur für **freigegebene Anwendungen** oder für Konten zu, die bei einer Anwendung als **Tester**
eingetragen sind. Deshalb legst du dir eine eigene (kostenlose) Anwendung an:

1. Im [Discord Developer Portal](https://discord.com/developers/applications) **New Application** anlegen, Name z. B.
   „OpenDeckN3“.
2. Unter **OAuth2**: **Client ID** kopieren, **Reset Secret** → **Client Secret** kopieren (wird nur einmal angezeigt).
3. Ebenfalls unter **OAuth2 → Redirects**: `http://localhost` eintragen und speichern (dort muss nichts laufen).
4. Falls beim Verbinden „nicht auf der Testerliste“ erscheint: unter **App Testers** das eigene Konto hinzufügen.
5. In OpenDeckN3: **Plugins → Discord** → Client-ID, Client-Secret eintragen → **Mit Discord verbinden**.
6. In Discord erscheint ein Fenster „OpenDeckN3 möchte …“ → **Autorisieren**. Der Status zeigt „Verbunden als …“.

Danach verbindet sich das Plugin bei jedem Start automatisch (gespeichertes Token, wird selbst erneuert). Client-Secret
und Tokens liegen nur lokal in `<config>/plugin-settings/de.opendeckn3.discord.json`. **Abmelden** löscht die Tokens.

## Gut zu wissen

- Discord erlaubt immer nur **einer** App gleichzeitig, Spracheinstellungen per RPC zu ändern. Beendet sich
  OpenDeckN3, setzt Discord die per Taste geänderten Spracheinstellungen (z. B. Stummschaltung) zurück.
- **Kamera, Bildschirmfreigabe und Soundboard** nutzen RPC-Befehle, die Discord nicht offiziell dokumentiert
  (`TOGGLE_VIDEO`, `TOGGLE_SCREENSHARE`, `GET_SOUNDBOARD_SOUNDS`/`PLAY_SOUNDBOARD_SOUND`). Verweigert Discord die
  zugehörigen Berechtigungen, funktionieren alle anderen Aktionen trotzdem.
- **Drücken zum Sprechen** arbeitet über die Stummschaltung: In Discord „Sprachaktivierung“ nutzen und das Mikro
  stumm schalten – die Taste hebt die Stummschaltung auf, solange sie gedrückt ist.
- Nicht freigegebene Anwendungen sind bei Discord auf 50 Tester begrenzt – für den eigenen Rechner reicht das.

## Technik

```text
OpenDeckN3 ──WebSocket (Stream-Deck-SDK)──► opendeckn3-discord ──IPC (RPC v1)──► Discord-App
                                                   │
                                                   └── HTTPS discord.com/api/oauth2/token (Code → Token)
```

Ablauf: Handshake (`client_id`) → `AUTHORIZE` (Bestätigungsfenster, liefert einen Code) → Token-Tausch per OAuth2 →
`AUTHENTICATE`. Danach `SUBSCRIBE` auf `VOICE_SETTINGS_UPDATE`, `VOICE_CHANNEL_SELECT`, `NOTIFICATION_CREATE`,
`VIDEO_STATE_UPDATE`, `SCREENSHARE_STATE_UPDATE`, damit die Tasten den Zustand in Discord live zeigen. Befehle:
`SET_VOICE_SETTINGS`, `SELECT_VOICE_CHANNEL`, `SELECT_TEXT_CHANNEL`, `SET_USER_VOICE_SETTINGS`, `GET_VOICE_SETTINGS`,
`GET_SELECTED_VOICE_CHANNEL` sowie die oben genannten undokumentierten.

Entwickeln/Testen ohne Discord: `cargo test -p opendeckn3-discord` (simuliertes Discord über einen In-Memory-Stream
und einen lokalen Token-Server). `OPENDECKN3_DISCORD_API` lenkt die OAuth2-Anfragen auf einen Test-Server um.

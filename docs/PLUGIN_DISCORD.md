# Discord-Plugin

Steuert die **Discord-Desktop-App** vom TreasLin N3 aus – über Discords lokale RPC-Schnittstelle (IPC: Named Pipe
`\\.\pipe\discord-ipc-0` unter Windows, Unix-Socket `discord-ipc-0` unter Linux/macOS). Es wird kein Discord-Passwort und
kein Benutzer-Token ausgelesen; der Zugriff wird über Discords eigenes Bestätigungsfenster erteilt.

Quellcode: [`plugins/discord`](../plugins/discord) (Rust, ein einzelnes Programm). Nur die Aktion „Online-Status“
braucht zusätzlich das Discord Social SDK (siehe unten).

## Aktionen

| Aktion | Taste | Drehregler | Anzeige auf der Taste |
| --- | --- | --- | --- |
| Mikrofon stummschalten | umschalten / ein / aus | – | rot, wenn stumm |
| Kopfhörer stummschalten | umschalten / ein / aus | – | rot, wenn taub geschaltet |
| Sprachkanal | betreten / verlassen (Server + Kanal aus Liste) | – | grün, wenn im Kanal |
| Textkanal | Kanal öffnen (Server + Kanal aus Liste) | – | – |
| Benachrichtigung | Zähler zurücksetzen | – | Anzahl neuer Benachrichtigungen |
| Benutzer-Lautstärkeregelung | lauter / leiser / stumm (Person aus dem Sprachkanal) | drehen = 0–200 %, drücken = stumm | Lautstärke in % |
| Lautstärkeregelung | lauter / leiser / stumm (Eingabe oder Ausgabe) | drehen = Lautstärke, drücken = stumm | Lautstärke in % |
| Sprachboard | Sound abspielen (aus Liste) | – | – |
| Audiogerät einstellen | Eingabe-/Ausgabegerät wählen (aus Liste) | – | grün, wenn aktiv |
| Kamera ein-/ausschalten | umschalten | – | grün, wenn an |
| Bildschirmfreigabe ein-/ausschalten | umschalten | – | grün, wenn aktiv |
| Drücken zum Sprechen | solange gedrückt: Mikro an, danach wieder stumm | – | grün, solange gedrückt |
| Sprachaktivierung umschalten | Sprachaktivierung ↔ Push-to-Talk | – | Tastatur-Symbol bei Push-to-Talk |
| Online-Status | Status setzen; mit „Umschalten mit“ zwischen zwei Status wechseln | – | aktueller Status (grün, gelb, rot, grau), auch bei Änderungen in Discord |

Server, Kanäle, Personen (im eigenen Sprachkanal), Soundboard-Sounds und Audiogeräte wählst du im Inspektor aus
Listen, die das Plugin live bei Discord abfragt – IDs oder der Entwicklermodus sind nicht nötig.

## Verbinden

**Schnell (Standard):** Plugins → Discord → **Mit Discord verbinden** → in Discord **Autorisieren**. Fertig – auch der
Inspektor jeder Discord-Taste zeigt den Status und den Knopf. Dafür nutzt das Plugin die gemeinsame Discord-Anwendung
„OpenDeckN3“.

Discord erlaubt die Sprachsteuerung nur Anwendungen, die Discord freigeschaltet hat (wie Elgato oder VSD Craft), oder
Konten, die bei einer Anwendung als **Tester** eingetragen sind (höchstens 50). Die Schnell-Anmeldung funktioniert
deshalb für den Betreuer der OpenDeckN3-Anwendung und die dort eingetragenen Tester. Alle anderen nutzen eine eigene
Anwendung:

**Eigene Discord-Anwendung (funktioniert immer, einmalig ca. 3 Minuten):**

1. Im [Discord Developer Portal](https://discord.com/developers/applications) **New Application** anlegen, Name z. B.
   „OpenDeckN3“.
2. Unter **OAuth2**: **Client ID** kopieren, **Public Client** einschalten (dann ist kein Secret nötig – sonst
   **Reset Secret** und das Secret kopieren).
3. Unter **OAuth2 → Redirects**: `http://localhost` eintragen und speichern (dort muss nichts laufen).
4. Erscheint beim Verbinden „nicht auf der Testerliste“: unter **App Testers** das eigene Konto hinzufügen.
5. In OpenDeckN3: Plugins → Discord → Anmeldung **„Eigene Discord-Anwendung“** → Client-ID (und ggf. Secret) eintragen
   → **Mit Discord verbinden** → in Discord **Autorisieren**.

Danach verbindet sich das Plugin bei jedem Start automatisch (gespeichertes Token, wird selbst erneuert). Tokens und
ein eventuelles Secret liegen nur lokal in `<config>/plugin-settings/de.opendeckn3.discord.json`. **Abmelden** löscht
die Tokens.

### Für den Betreuer: Schnell-Anmeldung einrichten

Die Anwendung „OpenDeckN3“ (Client-ID `1557797665281147052`, eingetragen als `QUICK_CLIENT_ID` in
`plugins/discord/src/main.rs`) braucht **Public Client** und den Redirect `http://localhost`. Wer die Schnell-Anmeldung
nutzen soll, wird dort unter **App Testers** hinzugefügt. Für mehr als
50 Personen bräuchte die Anwendung eine RPC-Freigabe von Discord. Zum Testen lässt sich die ID ohne Neubau per
Umgebungsvariable `OPENDECKN3_DISCORD_CLIENT_ID` setzen.

## Gut zu wissen

- Discord erlaubt immer nur **einer** App gleichzeitig, Spracheinstellungen per RPC zu ändern. Beendet sich
  OpenDeckN3, setzt Discord die per Taste geänderten Spracheinstellungen (z. B. Stummschaltung) zurück.
- **Kamera, Bildschirmfreigabe und Soundboard** nutzen RPC-Befehle, die Discord nicht offiziell dokumentiert
  (`TOGGLE_VIDEO`, `TOGGLE_SCREENSHARE`, `GET_SOUNDBOARD_SOUNDS`/`PLAY_SOUNDBOARD_SOUND`). Verweigert Discord die
  zugehörigen Berechtigungen, funktionieren alle anderen Aktionen trotzdem.
- **Drücken zum Sprechen** arbeitet über die Stummschaltung: In Discord „Sprachaktivierung“ nutzen und das Mikro
  stumm schalten – die Taste hebt die Stummschaltung auf, solange sie gedrückt ist.
- Nicht freigegebene Anwendungen sind bei Discord auf 50 Tester begrenzt – für den eigenen Rechner reicht das.
- Den **Online-Status** gibt es nicht über RPC. Das Plugin nutzt dafür das **Discord Social SDK**: Die Anwendung
  braucht dort ein aktiviertes Social SDK, die Anmeldung den Scope `sdk.social_layer_presence`. Wer vor dieser Aktion
  verbunden war, muss sich einmal **abmelden** und neu verbinden. Der Status bleibt nach dem Setzen bestehen, auch
  wenn OpenDeckN3 beendet wird.

## Technik

```text
OpenDeckN3 ──WebSocket (Stream-Deck-SDK)──► opendeckn3-discord ──IPC (RPC v1)──► Discord-App
                                                   │
                                                   ├── HTTPS discord.com/api/oauth2/token (Code → Token)
                                                   └── Discord Social SDK (nur Online-Status)
```

Ablauf: Handshake (`client_id`) → `AUTHORIZE` (Bestätigungsfenster, liefert einen Code) → Token-Tausch per OAuth2 →
`AUTHENTICATE`. Danach `SUBSCRIBE` auf `VOICE_SETTINGS_UPDATE`, `VOICE_CHANNEL_SELECT`, `NOTIFICATION_CREATE`,
`VIDEO_STATE_UPDATE`, `SCREENSHARE_STATE_UPDATE`, damit die Tasten den Zustand in Discord live zeigen. Befehle:
`SET_VOICE_SETTINGS`, `SELECT_VOICE_CHANNEL`, `SELECT_TEXT_CHANNEL`, `SET_USER_VOICE_SETTINGS`, `GET_VOICE_SETTINGS`,
`GET_SELECTED_VOICE_CHANNEL`, für die Auswahllisten `GET_GUILDS`, `GET_CHANNELS` sowie die oben genannten
undokumentierten.

**Online-Status:** Das Social SDK ist proprietär und liegt nicht im Repository. Das Plugin lädt
`discord_partner_sdk.dll` (Linux: `libdiscord_partner_sdk.so`) zur Laufzeit aus seinem eigenen Ordner
([`social.rs`](../plugins/discord/src/social.rs)). Fehlt die Datei, meldet nur diese Taste einen Fehler. Solange
eine Status-Taste existiert, hält ein eigener Thread eine SDK-Verbindung: `UpdateToken` (Bearer, das Token der
RPC-Anmeldung) → `Connect` → `Ready`. Den eigenen Status liest er mit `GetCurrentUser`, Änderungen (auch in Discord
selbst) meldet `SetUserUpdatedCallback`; ein Tastendruck ruft `SetOnlineStatus`. Eine Verbindung pro Abfrage wäre
langsamer und liefe in Discords Verbindungslimits. Die Erneuerung über den SDK-eigenen Token-Endpunkt lehnt Discord für diese Anwendung
ab (`invalid_client`); deshalb erneuert weiter der RPC-Weg über `discord.com` das gemeinsame Token. Die DLL liegt mit den
Lizenzhinweisen des SDK AES-verschlüsselt in `third_party/discord-social-sdk.7z`. Der Release-Workflow entpackt sie mit
dem Secret `DISCORD_SOCIAL_SDK_PASSWORD`, prüft ihre SHA-256 und legt sie ins Windows-Paket des Plugins (siehe
`.github/workflows/windows.yml`). Neue SDK-Version: Archiv mit `7z a -t7z -mhe=on -p<Passwort>` neu packen und die
Prüfsumme im Workflow anpassen. Das Verlinken mit dem SDK erlaubt eine Zusatzerlaubnis nach GPLv3 §7 am Ende
von [`LICENSE`](../LICENSE).

Entwickeln/Testen ohne Discord: `cargo test -p opendeckn3-discord` (simuliertes Discord über einen In-Memory-Stream
und einen lokalen Token-Server). `OPENDECKN3_DISCORD_API` lenkt die OAuth2-Anfragen auf einen Test-Server um.

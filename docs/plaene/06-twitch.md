# 06 – Twitch

- UUID `de.opendeckn3.twitch`, Ordner `plugins/twitch`, Win + Linux. HTTP mit `reqwest` (+ `json`).
- Anmeldung: **Device Code Grant** (öffentlicher Client, kein Secret).
  1. Nutzer registriert eine App unter dev.twitch.tv (Typ „Public“, Redirect `http://localhost`), trägt `clientId` ein.
  2. Knopf „Anmelden“ → `POST https://id.twitch.tv/oauth2/device` (`client_id`, `scopes`) → `openUrl` auf
     `verification_uri`, `user_code` im `info`-Feld `status` anzeigen.
  3. `POST https://id.twitch.tv/oauth2/token` mit `grant_type=urn:ietf:params:oauth:grant-type:device_code` im
     Abstand `interval` abfragen bis Erfolg/Ablauf.
  4. `access_token`, `refresh_token`, Ablaufzeit, `user_id` (`GET /helix/users`) in den globalen Einstellungen
     speichern. Bei 401 einmal erneuern (`grant_type=refresh_token`), sonst Status „Bitte neu anmelden“.
- Scopes: `channel:manage:broadcast clips:edit moderator:manage:chat_settings channel:edit:commercial`.
- Alle Helix-Aufrufe mit Headern `Authorization: Bearer …` und `Client-Id`.

## Aktionen

| UUID-Suffix | Name | Aufruf | Verhalten |
| --- | --- | --- | --- |
| `marker` | Marker setzen | `POST /helix/streams/markers` (`user_id`, optional `description`) | `showOk`; offline → `showAlert` |
| `clip` | Clip erstellen | `POST /helix/clips?broadcaster_id=` | Einstellung `open` ja/nein: Bearbeiten-Link öffnen |
| `chatmode` | Chat-Modus | `PATCH /helix/chat/settings?broadcaster_id=&moderator_id=` | `mode`: `slow_mode`, `follower_mode`, `subscriber_mode`, `emote_mode`, `unique_chat_mode`; umschalten, Zustand 1 = an |
| `viewers` | Zuschauer | `GET /helix/streams?user_id=` (alle 60 s) | Titel = Zuschauerzahl, offline → „offline“; Taste öffnet den Kanal |
| `ad` | Werbung | `POST /helix/channels/commercial` | `length` 30/60/90/120/180 s |

Chat-Modus-Zustand beim Erscheinen mit `GET /helix/chat/settings` lesen.

## Tests

- Token-Ablauf: erneuern, wenn weniger als 60 s übrig.
- Device-Flow-Antworten `authorization_pending`, `slow_down` (Intervall +5 s), `expired_token`.
- Zuschauerzahl formatieren (`1,2 Tsd.` ab 1000).

## Doku

`docs/PLUGIN_TWITCH.md` mit Schritt-für-Schritt-Anleitung zur App-Registrierung.

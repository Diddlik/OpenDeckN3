# 07 – YouTube Live

- UUID `de.opendeckn3.youtube`, Ordner `plugins/youtube`, Win + Linux. HTTP mit `reqwest` (+ `json`).
- Anmeldung: Google **OAuth für TV und Geräte mit begrenzter Eingabe** (Device Flow).
  1. Nutzer legt in der Google Cloud Console ein Projekt an, aktiviert „YouTube Data API v3“, erstellt einen
     OAuth-Client vom Typ „TVs and Limited Input devices“, trägt `clientId` und `clientSecret` ein (Google behandelt
     dieses Secret bei diesem Typ nicht als vertraulich).
  2. `POST https://oauth2.googleapis.com/device/code` (`client_id`, `scope=https://www.googleapis.com/auth/youtube`)
     → `openUrl` auf `verification_url`, `user_code` im Status anzeigen.
  3. `POST https://oauth2.googleapis.com/token` mit `grant_type=urn:ietf:params:oauth:grant-type:device_code` abfragen.
  4. Tokens wie bei Twitch speichern und erneuern.
- Kontingent: 10 000 Einheiten/Tag. Abfragen höchstens alle 60 s und nur, solange eine Anzeige-Aktion sichtbar ist.

## Aktionen

| UUID-Suffix | Name | Aufruf | Verhalten |
| --- | --- | --- | --- |
| `status` | Live-Status | `GET liveBroadcasts?part=snippet,status&mine=true&broadcastStatus=active` | Zustand 1 = live; Taste öffnet das Studio (`https://studio.youtube.com`) |
| `viewers` | Zuschauer | dann `GET videos?part=liveStreamingDetails&id=` | Titel = `concurrentViewers`, offline → „offline“ |
| `chat` | Chat-Nachricht | `POST liveChat/messages?part=snippet` mit `liveChatId` der aktiven Übertragung | Text aus Einstellung `message`; `showOk` |

`status` und `viewers` teilen sich eine Abfrage pro Minute.

## Tests

- Auswahl der aktiven Übertragung aus der Liste.
- Fehlerfälle: kein aktiver Stream, Kontingent erschöpft (`quotaExceeded` → Status-Text, Abfragen 1 h aussetzen).

## Doku

`docs/PLUGIN_YOUTUBE.md` mit Anleitung zur Google-Cloud-Einrichtung. Für die eigene Nutzung bleibt die App im Modus
„Testing“; der Nutzer trägt sich selbst als Testnutzer ein.

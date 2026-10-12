# 14 – Spotify

Ergänzt Plan 02 um das, was Medientasten nicht können. Play/Pause, Titel und Cover bleiben im Medien-Plugin.

- UUID `de.opendeckn3.spotify`, Ordner `plugins/spotify`, Win + Linux. HTTP mit `reqwest` (+ `json`).
- Anmeldung: Authorization Code mit **PKCE** (kein Secret).
  1. Nutzer registriert eine App im Spotify Developer Dashboard, Redirect-URI `http://127.0.0.1:57140/callback`,
     trägt `clientId` ein.
  2. Knopf „Anmelden“ → Plugin lauscht kurz auf `127.0.0.1:57140` (nur eine Anfrage, 5 min Zeitlimit, `state` prüfen)
     und öffnet die Anmeldeseite per `openUrl`.
  3. Code gegen Token tauschen, Tokens speichern und erneuern (PKCE-Verifier wie in `plugins/discord/src/auth.rs`).
- Scopes: `user-read-playback-state user-modify-playback-state user-library-read user-library-modify playlist-modify-private playlist-modify-public`.
- Stand Februar 2026 (Migrationsleitfaden von Spotify): Apps im Entwicklungsmodus brauchen einen Premium-Account des
  App-Besitzers, höchstens 5 eingetragene Nutzer, Kontingent pro Konto (429 mit `QUOTA_EXCEEDED`). `/me/tracks` und
  `/playlists/{id}/tracks` gibt es dort nicht mehr; Ersatz siehe Tabelle. Steuerbefehle verlangen Premium.

## Aktionen

| UUID-Suffix | Name | Aufruf | Verhalten |
| --- | --- | --- | --- |
| `like` | Gefällt mir | `GET /me/library/contains`, `PUT`/`DELETE /me/library` (Spotify-URIs `spotify:track:<id>`) | Zustand 1 = gespeichert, umschalten |
| `addtoplaylist` | Zur Playlist hinzufügen | `POST /playlists/{id}/items` | Playlist per `source: "playlists"` (`GET /me/playlists`, nur eigene/gemeinsame); `showOk` |
| `shuffle` | Zufall | `PUT /me/player/shuffle` | Zustand 1 = an |
| `repeat` | Wiederholen | `PUT /me/player/repeat` | `off` → `context` → `track`, Zustand 0/1/2 |
| `device` | Gerät wechseln | `PUT /me/player` (`device_ids`) | Gerät per `source: "devices"` |

Aktueller Titel und Zustand: `GET /me/player` alle 5 s, nur solange eine Taste sichtbar ist.

## Tests

- PKCE-Challenge (gleiches Verfahren wie Discord; ggf. Code teilen statt kopieren, wenn es ohne neues Crate geht).
- Callback-Anfrage parsen (`code`, `state`, `error`).
- Repeat-Reihenfolge.

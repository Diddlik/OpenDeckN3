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
- Steuerbefehle verlangen Spotify Premium; App im Entwicklungsmodus erlaubt nur wenige eingetragene Nutzer. Beides
  in der Doku erklären und vor der Umsetzung die aktuellen Spotify-Bedingungen prüfen.

## Aktionen

| UUID-Suffix | Name | Aufruf | Verhalten |
| --- | --- | --- | --- |
| `like` | Gefällt mir | `GET /me/tracks/contains`, `PUT`/`DELETE /me/tracks` | Zustand 1 = gespeichert, umschalten |
| `addtoplaylist` | Zur Playlist hinzufügen | `POST /playlists/{id}/tracks` | Playlist per `source: "playlists"`; `showOk` |
| `shuffle` | Zufall | `PUT /me/player/shuffle` | Zustand 1 = an |
| `repeat` | Wiederholen | `PUT /me/player/repeat` | `off` → `context` → `track`, Zustand 0/1/2 |
| `device` | Gerät wechseln | `PUT /me/player` (`device_ids`) | Gerät per `source: "devices"` |

Aktueller Titel und Zustand: `GET /me/player` alle 5 s, nur solange eine Taste sichtbar ist.

## Tests

- PKCE-Challenge (gleiches Verfahren wie Discord; ggf. Code teilen statt kopieren, wenn es ohne neues Crate geht).
- Callback-Anfrage parsen (`code`, `state`, `error`).
- Repeat-Reihenfolge.

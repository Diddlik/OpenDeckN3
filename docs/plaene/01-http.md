# 01 – HTTP-Anfrage / Webhook

Allgemeine HTTP-Aktionen für n8n, Zapier, IFTTT, eigene Server. Home Assistant hat eigene Webhooks; dieses Plugin ist
für alles andere.

- UUID `de.opendeckn3.http`, Ordner `plugins/http`, Win + Linux.
- Abhängigkeiten: SDK, `anyhow`, `serde_json`, `tokio`, `reqwest` (siehe Checkliste).

## Aktionen

### `de.opendeckn3.http.request` – „Anfrage senden“ (Keypad, Encoder)

| Feld | Typ | Bemerkung |
| --- | --- | --- |
| `method` | select | GET, POST, PUT, PATCH, DELETE; Standard POST |
| `url` | text | nur `http://` und `https://`, sonst Fehler |
| `headers` | textarea | eine Zeile pro Header, `Name: Wert` |
| `body` | textarea | wird unverändert gesendet |
| `contentType` | select | `application/json` (Standard), `text/plain`, `application/x-www-form-urlencoded` |
| `showResponse` | text | optional: JSON-Pointer (z. B. `/status`); Treffer wird als Titel gezeigt |

- Taste: Anfrage senden. 2xx → `showOk`, sonst `showAlert` und Log mit Statuscode.
- Drehregler: je Rastung eine Anfrage; `{{ticks}}` in URL und Body wird durch die Rastenzahl (±n) ersetzt.
  Drücken des Reglers sendet wie die Taste.
- Zeitlimit 10 s. Anfragen laufen in eigenen Tasks und blockieren die Hauptschleife nicht.

### `de.opendeckn3.http.poll` – „Wert anzeigen“ (Keypad)

| Feld | Typ | Bemerkung |
| --- | --- | --- |
| `url`, `headers` | wie oben | immer GET |
| `pointer` | text | JSON-Pointer; leer → ganze Antwort als Text (gekürzt auf 12 Zeichen) |
| `interval` | number | Sekunden, Standard 30, min 5 |
| `prefix`, `suffix` | text | z. B. `€` |
| `openUrl` | text | optional; Tastendruck öffnet diese URL (`openUrl`-Event) |

Fehler zeigen `–` als Titel und loggen einmal pro Fehlerwechsel (nicht bei jedem Poll).

## Tests

- Header-Zeilen parsen (Leerzeilen, fehlender Doppelpunkt → Fehler).
- `{{ticks}}`-Ersetzung.
- JSON-Pointer-Ausgabe: String ohne Anführungszeichen, Zahl, Bool, fehlend.
- URL-Schema-Prüfung.

## Doku

`docs/PLUGIN_HTTP.md` mit Beispielen für n8n- und IFTTT-Webhooks.

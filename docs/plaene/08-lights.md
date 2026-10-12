# 08 – Licht: Elgato Key Light, Philips Hue, WLED

Ein Plugin für Lampen im lokalen Netz, ohne Home Assistant. Gemeinsame Aktionen, die Lampenart wird pro Taste gewählt.

- UUID `de.opendeckn3.lights`, Ordner `plugins/lights`, Win + Linux. HTTP mit `reqwest` (+ `json`).

## Lampenarten

| Art | API | Helligkeit | Farbtemperatur |
| --- | --- | --- | --- |
| Elgato (Key Light, Ring Light) | `GET/PUT http://<ip>:9123/elgato/lights`, Körper `{"lights":[{"on":1,"brightness":50,"temperature":200}]}` | 3–100 | 143–344 (≈ 7000–2900 K) |
| Hue (Bridge, API v1) | `PUT http://<bridge>/api/<user>/lights/<id>/state` bzw. `/groups/<id>/action`, `{"on":true,"bri":1-254,"ct":153-500}` | 1–254 | 153–500 Mired |
| WLED | `GET/POST http://<ip>/json/state`, `{"on":true,"bri":0-255,"ps":<preset>}` | 0–255 | – |

Hue-Kopplung: globale Einstellung `hueBridge` (IP) und Knopf „Mit Bridge koppeln“ → Nutzer drückt den Knopf an der
Bridge → `POST /api` mit `{"devicetype":"opendeckn3#pc"}` (bis 30 s lang alle 2 s wiederholen) → `username` speichern.

## Aktionen

Gemeinsame Felder: `kind` (select `elgato`/`hue`/`wled`), `host` (IP, nicht bei Hue), `light`
(Hue: select mit `source: "hueLights"`, Lampen und Räume/Gruppen).

| UUID-Suffix | Name | Controller | Verhalten |
| --- | --- | --- | --- |
| `power` | Ein/Aus | Keypad | umschalten, Zustand 1 = an |
| `brightness` | Helligkeit | Encoder, Keypad | Drehen: `step` % (Standard 5), Drücken/Taste: ein/aus; Titel `60 %` |
| `temperature` | Farbtemperatur | Encoder | Drehen: ±100 K je Raste; Titel `4500 K`; WLED nicht unterstützt → „–“ |
| `scene` | Szene / Preset | Keypad | Hue: Szene (`source: "hueScenes"`, `PUT /groups/<g>/action {"scene":id}`); WLED: Preset-Nummer |

- Zustand alle 5 s lesen, damit Änderungen aus anderen Apps sichtbar werden.
- Drehen: Werte lokal sofort ändern und höchstens alle 100 ms senden (letzter Wert gewinnt), damit die Lampe nicht ruckelt.

## Tests

- Umrechnung Prozent ↔ Gerätewert je Art, Kelvin ↔ Elgato-Wert ↔ Mired, Klemmen auf die Grenzen.
- Hue-Kopplung: Antwort `link button not pressed` (Typ 101) → weiter warten.

# Licht-Plugin

Steuert **Elgato Key Light**, **Philips Hue** und **WLED** im lokalen Netz – ohne Home Assistant. Quellcode:
[`plugins/lights`](../plugins/lights). Windows und Linux.

## Einrichten

1. **Plugins → Katalog → Licht → Installieren.**
2. **IP-Adressen finden** (siehe unten). Die Adresse trägst du in jeder Taste ein, außer bei Hue.
3. **Nur für Hue:** unter **Plugins → Licht → Einstellungen** die IP der Bridge eintragen und **Mit Bridge koppeln**
   klicken (siehe unten).

### IP-Adressen finden

- **Elgato Key Light / Ring Light:** Die Lampe bekommt vom Router eine IP-Adresse. Sie steht in der Geräteliste deines
  Routers (Name meist „Elgato Key Light …“) oder im Elgato Control Center. Das Plugin findet Elgato-Lampen **nicht
  selbst**; die IP muss von Hand eingetragen werden. Die Lampe lauscht auf Port 9123.
- **Philips Hue:** Die IP der Bridge steht in der Hue-App unter *Einstellungen → Hue-Bridges*. Am besten reserviert
  dein Router der Bridge eine feste IP, damit sie sich nicht ändert.
- **WLED:** Die IP steht in der Info-Seite der WLED-Weboberfläche (im Browser öffnen, z. B. `http://192.168.1.60`)
  oder in der Geräteliste des Routers.

### Hue koppeln

1. Die IP der Bridge unter **Hue-Bridge (IP-Adresse)** eintragen.
2. Den runden Knopf oben auf der Bridge drücken.
3. Innerhalb von 30 Sekunden **Mit Bridge koppeln** klicken.

Der Status zeigt „Mit Hue-Bridge gekoppelt“, sobald es geklappt hat. Der Benutzername der Kopplung wird in den globalen
Einstellungen des Plugins gespeichert. Er ist ein Zugangsschlüssel: Gib ihn nicht weiter. Wenn die Kopplung nicht klappt,
steht der Grund im Status (z. B. „Knopf an der Bridge nicht innerhalb von 30 s gedrückt“). Eine neue Kopplung ist jederzeit
möglich.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Ein/Aus | umschalten | – | leuchtet, wenn an |
| Helligkeit | ein/aus | Helligkeit in Schritten (Standard 5 %) | `60 %`, bei aus `aus` |
| Farbtemperatur | – | ±100 K je Raste | `4500 K` |
| Szene / Preset | Hue: Szene aufrufen; WLED: Preset abspielen | – | – |

Jede Aktion hat die Felder **Lampenart** (Elgato, Philips Hue, WLED) und **IP-Adresse**. Bei Hue wählst du
stattdessen **Lampe oder Raum** aus einer Liste, die das Plugin von der Bridge holt. Ein Raum schaltet alle Lampen darin
gemeinsam. Hue-Szenen wählst du ebenfalls aus einer Liste; sie gehören zu einem Raum.

Die Taste bzw. der Drehregler zeigt den Zustand der Lampe. Alle 5 Sekunden wird der Zustand der sichtbaren Tasten
gelesen, damit Änderungen aus anderen Apps erscheinen. Beim Drehen ändert sich der Wert sofort auf dem Display; die Lampe
bekommt höchstens alle 100 ms einen neuen Wert, damit sie nicht ruckelt. Ist ein Gerät nicht erreichbar, zeigt die Taste
`offline`.

## Grenzen

- **WLED** hat keine Farbtemperatur. Die Aktion Farbtemperatur zeigt dort einen Fehler; für WLED-Strips gibt es nur
  Helligkeit, Ein/Aus und Presets.
- **Elgato** wird nicht automatisch gefunden; die IP muss eingetragen werden. Elgato-Lampen haben keine Szenen.
- **Hue-Szenen** werden über die Bridge aufgerufen (API v1). Lichtszenen ohne Raum werden über Gruppe 0 aufgerufen.
- Das Plugin spricht nur mit Geräten im selben lokalen Netz. Es gibt keine Anbindung an die Cloud.

## Technik

Gesprochen wird jeweils die lokale HTTP-API des Geräts:

- Elgato: `GET`/`PUT http://<ip>:9123/elgato/lights`
- Hue (API v1): `GET`/`PUT http://<bridge>/api/<benutzer>/lights/<id>/state`, `…/groups/<id>/action`, Szenen über
  `…/groups/<id>/action` mit `{"scene": …}`; Kopplung über `POST http://<bridge>/api`
- WLED: `GET`/`POST http://<ip>/json/state`

Zeitlimit je Anfrage 3 Sekunden. Tests ohne Geräte: `cargo test -p opendeckn3-lights` (Umrechnungen, Antworten,
Kopplung und Dropdowns).

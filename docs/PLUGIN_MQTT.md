# MQTT-Plugin

Verbindet OpenDeckN3 mit einem **MQTT-Broker** (z. B. Mosquitto). Damit steuerst du Geräte wie Zigbee2MQTT-Lampen,
Tasmota-Steckdosen, ioBroker, Node-RED und andere MQTT-Geräte. Quellcode: [`plugins/mqtt`](../plugins/mqtt).
Windows und Linux.

## Einrichten

1. **Plugins → Katalog → MQTT → Installieren.**
2. Unter **Plugins → MQTT → Einstellungen** den **Broker** (Host oder IP) eintragen. Port, Benutzername und Passwort
   nur, wenn dein Broker sie verlangt.
3. Das Feld **Status** zeigt, ob die Verbindung steht („Verbunden mit …“) oder warum nicht.

Die Verbindung gilt für alle MQTT-Tasten gemeinsam. Bricht sie ab, verbindet sich das Plugin alle 5 Sekunden neu.
Nach jeder Verbindung werden die nötigen Topics neu abonniert.

**Verschlüsselung (TLS):** Mit „Ja“ wird TLS mit den Zertifikaten des Systems genutzt (meist Port 8883). Eigene
CA-Zertifikate kann das Plugin nicht laden.

## Aktionen

| Aktion | Controller | Funktion |
| --- | --- | --- |
| **Nachricht senden** | Taste, Drehregler | Taste sendet die Nachricht. Der Drehregler ersetzt `{{ticks}}` im Inhalt durch die Rastenzahl (z. B. `-1` oder `2`) und sendet je Rastung. Bei einer Taste wird `{{ticks}}` durch `0` ersetzt. |
| **Schalter** | Taste | Abonniert das Status-Topic. Leuchtet, wenn der Wert dort dem Wert für „an“ entspricht. Die Taste sendet das Gegenteil an das Befehls-Topic. |
| **Wert anzeigen** | Taste | Zeigt die letzte Nachricht eines Topics als Titel, optional mit Suffix (z. B. `°C`). |

Die Aktion **Wert anzeigen** und der **Schalter** abonnieren ihr Topic nur, solange eine Taste mit ihnen sichtbar ist.
Mehrere Tasten mit demselben Topic nutzen ein gemeinsames Abo. Der letzte Wert bleibt gespeichert, damit eine neu
belegte Taste ihn sofort zeigt. Ist das Gerät nicht verbunden, zeigen Schalter und Werte `offline`.

### Werte aus JSON lesen

Viele Geräte senden JSON, z. B. `{"state":"ON","brightness":128}`. Mit dem Feld **Wert (JSON-Pointer)** wählst du den
Teil aus:

- `/state` → `ON`
- `/brightness` → `128`
- Leer → die ganze Nachricht als Text

Ist die Nachricht kein JSON, wird sie als Text gezeigt. Fehlt der Pointer in einer gültigen JSON-Nachricht, zeigt die
Taste `–`.

## Beispiele

### Zigbee2MQTT

Zigbee2MQTT veröffentlicht den Zustand eines Geräts als JSON unter `zigbee2mqtt/<Name>`, z. B.
`{"state":"ON"}`. Befehle nimmt es unter `zigbee2mqtt/<Name>/set/state` mit `ON` oder `OFF` entgegen.

**Schalter** für eine Lampe namens „Wohnzimmer“:

| Feld | Wert |
| --- | --- |
| Befehls-Topic | `zigbee2mqtt/Wohnzimmer/set/state` |
| Status-Topic | `zigbee2mqtt/Wohnzimmer` |
| Wert für „an“ / „aus“ | `ON` / `OFF` |
| Wert (JSON-Pointer) | `/state` |

**Wert anzeigen** für einen Temperatursensor:

| Feld | Wert |
| --- | --- |
| Topic | `zigbee2mqtt/Schlafzimmer` |
| Wert (JSON-Pointer) | `/temperature` |
| Suffix | `°C` |

**Helligkeit per Drehregler:** Aktion **Nachricht senden**, Topic `zigbee2mqtt/Wohnzimmer/set`, Nachricht
`{"brightness": {{ticks}}}`. Jede Rastung sendet eine Nachricht mit dem Schritt als Zahl.

### Tasmota

Tasmota hört auf `cmnd/<Gerät>/POWER` und meldet den Zustand unter `stat/<Gerät>/POWER` als reinen Text (`ON` oder
`OFF`). Den Zustand findest du unter dem Gerätenamen in der Tasmota-Weboberfläche.

**Schalter** für eine Steckdose mit dem Gerätenamen `steckdose`:

| Feld | Wert |
| --- | --- |
| Befehls-Topic | `cmnd/steckdose/POWER` |
| Status-Topic | `stat/steckdose/POWER` |
| Wert für „an“ / „aus“ | `ON` / `OFF` |
| Wert (JSON-Pointer) | leer |

## Grenzen

- Nur MQTT über TCP, optional mit TLS (Port 8883 oder der Port deines Brokers). WebSockets werden nicht unterstützt.
- Eigene CA-Zertifikate und Client-Zertifikate werden nicht unterstützt.
- Veröffentlicht werden Nachrichten mit QoS 0 oder 1. QoS 2 ist nicht verfügbar.
- Ein Topic mit Platzhaltern (`+`, `#`) lässt sich nicht veröffentlichen; für Abos sind sie erlaubt.
- Das Passwort steht in den globalen Einstellungen des Plugins und wird nicht ins Protokoll geschrieben.

## Technik

- Bibliothek: [`rumqttc`](https://crates.io/crates/rumqttc) mit `use-rustls` (TLS über rustls mit den Systemzertifikaten).
- Eine Verbindung für alle Tasten; die Ereignisschleife läuft in einer eigenen Aufgabe.
- Clean Session: nach jeder Verbindung werden alle benötigten Topics neu abonniert.

Tests ohne Broker: `cargo test -p opendeckn3-mqtt` (Wertauslese, Schalterzustand, Abo-Liste, Topic-Prüfung).

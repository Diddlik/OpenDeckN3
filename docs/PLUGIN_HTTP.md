# HTTP-Anfrage-Plugin

Sendet HTTP-Anfragen vom Deck aus, etwa an Webhooks von [n8n](https://n8n.io), [Zapier](https://zapier.com) oder
[IFTTT](https://ifttt.com) oder an einen eigenen Server. Zeigt außerdem Werte aus JSON-Antworten auf der Taste.
Home Assistant hat ein eigenes Plugin mit Webhooks, siehe [Home Assistant](PLUGIN_HOMEASSISTANT.md).
Quellcode: [`plugins/http`](../plugins/http). Windows und Linux.

## Einrichten

Es gibt keine globalen Einstellungen und keine Anmeldung. Die Adresse und Header trägst du direkt in der Aktion ein.
Nur `http://` und `https://` sind erlaubt.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Anfrage senden | sendet die Anfrage | sendet je Rastung eine Anfrage | Haken bei Erfolg, Warnung bei Fehler; optional ein Wert aus der Antwort als Titel |
| Wert anzeigen | öffnet optional eine Seite (`openUrl`) | – | Wert aus der Antwort, regelmäßig aktualisiert |

### Anfrage senden

| Feld | Bemerkung |
| --- | --- |
| Methode | GET, POST (Standard), PUT, PATCH, DELETE |
| Adresse | `{{ticks}}` wird beim Drehen durch die Rastenzahl ersetzt, z. B. `1` oder `-2` |
| Header | eine Zeile pro Header, `Name: Wert` |
| Inhalt | wird unverändert gesendet; `{{ticks}}` wie in der Adresse |
| Inhaltstyp | JSON (Standard), Text oder Formular (`application/x-www-form-urlencoded`) |
| Antwort anzeigen | optional ein JSON-Pointer, z. B. `/status`; der Wert wird als Titel gezeigt |

Eine Antwort mit Status 2xx gilt als Erfolg. Andere Statuscodes und Verbindungsfehler zeigen eine Warnung; der
Grund steht im Log des Dienstes. Die Anfrage hat ein Zeitlimit von 10 Sekunden und läuft in einer eigenen Aufgabe,
die Oberfläche bleibt also bedienbar.

### Wert anzeigen

| Feld | Bemerkung |
| --- | --- |
| Adresse, Header | wie bei der Anfrage; immer GET |
| Wert | JSON-Pointer, z. B. `/euro/kurs`. Leer: die ganze Antwort als Text, gekürzt auf 12 Zeichen |
| Intervall | Sekunden, Standard 30, mindestens 5 |
| Präfix, Suffix | werden um den Wert gesetzt, z. B. `€` oder `°C` |
| Beim Drücken öffnen | optionale http(s)-Adresse, die der Standardbrowser öffnet |

Ein Fehler (Verbindung, Statuscode, kein JSON, Pfad fehlt) zeigt `–` als Titel. Der Fehler wird einmal ins Log
geschrieben und erst wieder, wenn sich der Fehler ändert, nicht bei jeder Abfrage.

## Beispiele

### n8n-Webhook

1. In n8n einen Workflow mit dem Auslöser **Webhook** anlegen. Die Methode auf `POST` stellen. Die Test-URL oder
   Produktions-URL kopieren.
2. Auf dem Deck eine Taste **Anfrage senden** anlegen und die URL eintragen, z. B.
   `https://n8n.example.com/webhook/licht`.
3. Methode `POST`, Inhaltstyp `JSON`, Inhalt `{"quelle": "n3"}`. Im Workflow steht der Inhalt im Feld `body`.

Mit dem Drehregler: Adresse `https://n8n.example.com/webhook/lautstaerke?schritt={{ticks}}` sendet bei einer Rastung
nach rechts `?schritt=1`, nach links `?schritt=-1`.

### IFTTT-Webhook

1. In IFTTT einen Applet mit dem Dienst **Webhooks** anlegen. Der Auslöser ist „Receive a web request“ mit einem
   Ereignisnamen, z. B. `licht_an`.
2. Den Schlüssel unter **Webhooks → Documentation** kopieren. Die Adresse ist
   `https://maker.ifttt.com/trigger/licht_an/with/key/<DEIN-SCHLÜSSEL>`.
3. Auf dem Deck **Anfrage senden** mit Methode `POST` (oder `GET`) und dieser Adresse anlegen. Der Schlüssel steht
   in der Adresse; sie nicht weitergeben.

### Wert aus einer Antwort

Eine Abfrage liefert `{"status": "ok", "temp": {"aussen": 21.5}}`. Dann zeigt
**Wert anzeigen** mit Wert `/temp/aussen` und Suffix `°C` die Zahl `21.5°C`. Mit Wert `/status` steht `ok` auf der
Taste.

## Tests

`cargo test -p opendeckn3-http` prüft das Parsen der Header, das Ersetzen von `{{ticks}}`, die Ausgabe der
JSON-Werte und die Prüfung der Adressen. Die Tests sprechen nicht mit echten Diensten.

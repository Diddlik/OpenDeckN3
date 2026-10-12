# 09 – MQTT

Allgemeine Brücke zu Zigbee2MQTT, Tasmota, ioBroker, Node-RED und anderen MQTT-Geräten.

- UUID `de.opendeckn3.mqtt`, Ordner `plugins/mqtt`, Win + Linux.
- Abhängigkeit: `rumqttc` (mit `use-rustls` für TLS).
- Global: `host`, `port` (Standard 1883), `username`, `password` (password), `tls` (ja/nein), Status (`info`).
  Eine Verbindung für alle Tasten, automatische Wiederverbindung; Abos nach jeder Verbindung neu setzen.

## Aktionen

| UUID-Suffix | Name | Controller | Einstellungen | Verhalten |
| --- | --- | --- | --- | --- |
| `publish` | Nachricht senden | Keypad, Encoder | `topic`, `payload`, `qos` (0/1), `retain` | Taste: senden. Drehen: `{{ticks}}` im Payload ersetzen und senden |
| `switch` | Schalter | Keypad | `commandTopic`, `stateTopic`, `payloadOn` (`ON`), `payloadOff` (`OFF`), `pointer` | abonniert `stateTopic`; Zustand 1 wenn Wert = `payloadOn`; Taste sendet das Gegenteil |
| `value` | Wert anzeigen | Keypad | `topic`, `pointer` (JSON-Pointer, optional), `suffix` | letzter Wert als Titel |

Zigbee2MQTT liefert JSON (`{"state":"ON"}`) → `pointer` `/state`.

## Tests

- Wert aus Payload holen: Text, JSON mit Pointer, ungültiges JSON → Rohtext.
- Abo-Liste aus den sichtbaren Tasten ableiten (keine doppelten Abos).

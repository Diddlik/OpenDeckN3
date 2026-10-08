# OBS-Studio-Plugin

Steuert **OBS Studio** über **obs-websocket** (Protokoll v5) – die Schnittstelle ist seit OBS 28 fest eingebaut.
Quellcode: [`plugins/obs`](../plugins/obs) (Rust, ein einzelnes Programm).

## Verbinden

1. In OBS: **Werkzeuge → WebSocket-Servereinstellungen → „WebSocket-Server aktivieren“** (einmalig).
2. In OpenDeckN3: **Plugins → Katalog → OBS Studio → Installieren**.
3. Fertig – mit **Verbindung: Automatisch** liest das Plugin Port und Passwort direkt aus den OBS-Einstellungen
   (`obs-studio/plugin_config/obs-websocket/config.json`) und verbindet sich mit dem OBS auf diesem PC.

**Manuell** (OBS auf einem anderen PC oder eigene Einstellungen): Adresse, Port (Standard 4455) und Passwort eintragen –
zu finden in OBS unter *WebSocket-Servereinstellungen → Verbindungsinformationen anzeigen*. Läuft OBS noch nicht,
versucht das Plugin alle 5 Sekunden neu zu verbinden.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Verbindung | neu verbinden | – | grün, wenn verbunden |
| Aufnahme | kurz/lang getrennt belegbar: starten/stoppen, Pause | – | rot + Laufzeit (`00:12:34`, ⏸ bei Pause) |
| Stream | starten/stoppen (lang drücken z. B. = stoppen) | – | rot + Laufzeit |
| Szene | Szene wechseln (Programm oder Vorschau im Studio-Modus) | – | leuchtet bei aktiver Szene |
| Quelle ein/aus | Quelle in einer Szene ein-/ausblenden | – | leuchtet, wenn sichtbar |
| Filter | Filter einer Quelle ein/aus | – | leuchtet, wenn aktiv |
| Audio | stumm / lauter / leiser | drehen = Lautstärke (dB), drücken = stumm | rot bei stumm, optional dB |
| Studio-Modus | an/aus oder Übergang auslösen | – | leuchtet im Studio-Modus |
| Übergang | Szenenübergang (und Dauer) wählen | – | leuchtet beim aktiven Übergang |
| Leistung | – | – | CPU, FPS, Arbeitsspeicher, ausgelassene Frames, freier Speicher |
| Wiederholungspuffer | speichern / starten / stoppen | – | leuchtet, wenn aktiv |
| Virtuelle Kamera | starten/stoppen | – | leuchtet, wenn aktiv |
| Makro | mehrere OBS-Befehle nacheinander | – | – |

Szenen, Quellen, Filter, Audioquellen und Übergänge wählst du aus Listen, die das Plugin live bei OBS abfragt.

### Makros

Eine Zeile pro Schritt – ein Befehl aus dem
[obs-websocket-Protokoll](https://github.com/obsproject/obs-websocket/blob/master/docs/generated/protocol.md) mit
optionalen JSON-Daten oder eine Pause:

```text
SetCurrentProgramScene {"sceneName": "Intro"}
wait 2000
StartRecord
ToggleInputMute {"inputName": "Mikrofon"}
```

## Technik

Verbindung per WebSocket zu `ws://<adresse>:<port>`; Anmeldung mit
`base64(sha256(base64(sha256(passwort + salt)) + challenge))`. Das Plugin abonniert die Ereignisse von OBS
(Szenen, Ausgaben, Eingänge, Filter, Szenenelemente …), damit die Tasten jede Änderung in OBS sofort zeigen.
Tests ohne OBS: `cargo test -p opendeckn3-obs` (simulierter obs-websocket-Server).

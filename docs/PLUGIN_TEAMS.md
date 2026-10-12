# Microsoft-Teams-Plugin

Steuert Besprechungen im neuen Microsoft Teams vom Deck aus: Mikrofon, Kamera, Hand heben, Hintergrundunschärfe,
Reaktionen und Verlassen. Quellcode: [`plugins/teams`](../plugins/teams). Nur Windows.

## Einrichten

1. In **neuem Teams** öffnen: Einstellungen → Datenschutz → **Drittanbieter-App-API verwalten** und die API einschalten.
2. Eine Besprechung starten oder beitreten.
3. Eine Taste des Plugins drücken. Teams fragt, ob OpenDeckN3 die Besprechung steuern darf: zustimmen.
   Das Plugin speichert den Kopplungsschlüssel in den globalen Einstellungen und sendet ihn ab dann bei jeder
   Verbindung mit. Weitere Fragen gibt es danach nicht.

Ist Teams nicht gestartet, versucht das Plugin alle 5 Sekunden erneut zu verbinden. Der Status zeigt „Verbunden mit
Teams“, sobald die Verbindung steht.

## Aktionen

| Aktion | Taste | Anzeige |
| --- | --- | --- |
| Mikrofon | schaltet stumm oder laut | leuchtet, solange das Mikrofon stumm ist |
| Kamera | schaltet die Kamera ein oder aus | leuchtet, solange die Kamera an ist |
| Hand heben | hebt oder senkt die Hand | leuchtet, solange die Hand oben ist |
| Hintergrund unscharf | schaltet die Hintergrundunschärfe um | leuchtet, solange sie an ist |
| Reaktion | sendet die gewählte Reaktion (Daumen hoch, Herz, Applaus, Lachen oder Überrascht) | – |
| Verlassen | verlässt die laufende Besprechung | – |

Außerhalb einer Besprechung zeigen alle Tasten „–“. Ein Druck meldet dann einen Fehler (`showAlert`), ebenso ein Druck
auf eine Aktion, die Teams in der aktuellen Besprechung nicht erlaubt (etwa wenn der Veranstalter das Stummschalten
der Teilnehmer gesperrt hat).

## Grenzen

- Nur das neue Teams unter Windows. Klassisches Teams und Teams für Linux stellen die API nicht bereit.
- Die Steuerung funktioniert nur während einer Besprechung. Die Zustände kommen direkt von Teams.
- Teams muss laufen und die Drittanbieter-App-API eingeschaltet haben.

## Technik

Teams stellt auf dem Rechner einen WebSocket unter `ws://127.0.0.1:8124` bereit. Das Plugin verbindet sich mit
`protocol-version`, `manufacturer`, `device`, `app` und `app-version` in der Adresse; `token` wird ergänzt, sobald ein
Kopplungsschlüssel gespeichert ist.

Befehl (eine JSON-Nachricht pro Taste):

```json
{ "action": "toggle-mute", "parameters": {}, "requestId": 1 }
```

Teams meldet den Zustand mit `meetingUpdate` (`meetingState` und `meetingPermissions`). Mit `tokenRefresh` schickt es
den Kopplungsschlüssel. Andere Nachrichten, etwa Antworten auf Befehle, werden ignoriert.

Tests: `cargo test -p opendeckn3-teams`.

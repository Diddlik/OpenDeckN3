# Timer-Plugin

Countdown, Pomodoro-Technik und Stoppuhr auf Tasten und Drehreglern. Laufende Timer bleiben beim Seiten- und
Profilwechsel erhalten. Quellcode: [`plugins/timer`](../plugins/timer). Windows und Linux.

## Einrichten

Es gibt keine globalen Einstellungen und keine Anmeldung. Die Zeiten stellst du direkt in der Aktion ein.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Countdown | startet oder pausiert; langer Druck setzt zurück | stellt die Minuten ein (nur im Stillstand) | Restzeit, z. B. `04:59`; am Ende `00:00` mit blinkendem Titel und Warnung |
| Pomodoro | startet oder pausiert; langer Druck setzt zurück | – | Phase und Restzeit, z. B. `Fokus 24:10` oder `Pause 04:59` |
| Stoppuhr | startet oder pausiert; langer Druck setzt auf `00:00` zurück | – | verstrichene Zeit, ab einer Stunde `1:02:03` |

Ein langer Druck dauert mindestens 0,6 Sekunden.

### Countdown

Standard sind 5 Minuten. Der Drehregler verstellt die Minuten um je eine Minute, solange der Countdown steht. Läuft
er, ändert das Drehen nichts. Ist die Zeit abgelaufen, bleibt die Taste auf `00:00`, zeigt den zweiten Zustand und
meldet sich mit einer Warnung. Der Titel blinkt 10 Sekunden lang. Ein Druck danach startet den Countdown neu.

### Pomodoro

Fokus- und Pausenphasen wechseln sich ab: Fokus, kurze Pause, Fokus, … Nach der letzten Fokus-Phase eines Zyklus folgt
die lange Pause. Standard: 25 Minuten Fokus, 5 Minuten kurze Pause, 15 Minuten lange Pause, 4 Fokus-Phasen pro Zyklus.
Am Ende einer Phase hält der Timer an und springt zur nächsten Phase; gestartet wird sie mit einem Druck. Die Taste
zeigt im Fokus den ersten Zustand, in einer Pause den zweiten.

## Grenzen

- Die Zeit läuft mit der Systemuhr weiter. Wird der Rechner im Ruhezustand ausgeschaltet, sind die Minuten dazwischen
  verstrichen.
- Der Titel wird einmal pro Sekunde aktualisiert.

## Technik

Ein laufender Timer speichert seine Zustände in den Einstellungen der Taste (`setSettings`): Startzeit als Unix-Zeit in
Millisekunden (`_since`), bisher verstrichene Zeit (`_elapsed`) und die Pomodoro-Phase (`_phase`). Die Schlüssel mit
Unterstrich gehören dem Plugin. Beim Erscheinen der Taste liest das Plugin sie wieder ein.

Tests: `cargo test -p opendeckn3-timer`.

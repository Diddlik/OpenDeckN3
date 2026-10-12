# 10 – Microsoft Teams

Steuert Besprechungen im neuen Teams über die lokale Drittanbieter-API (in Teams unter Einstellungen → Datenschutz →
„Drittanbieter-App-API verwalten“ einschalten).

- UUID `de.opendeckn3.teams`, Ordner `plugins/teams`, nur Windows (Teams für Linux hat die API nicht; Plugin baut
  dort, Aktionen melden „nur für Windows“).
- WebSocket `ws://127.0.0.1:8124?token=<token>&protocol-version=2.0.0&manufacturer=OpenDeckN3&device=N3&app=OpenDeckN3&app-version=<ver>`
  mit `tokio-tungstenite` (Workspace).
- Kopplung: Ohne Token verbinden; beim ersten Befehl während einer Besprechung fragt Teams nach Erlaubnis und schickt
  `{"tokenRefresh":"…"}` → Token in den globalen Einstellungen speichern und künftig mitsenden.
- Befehl: `{"action":"toggle-mute","parameters":{},"requestId":<n>}`.
- Zustand: Nachrichten `{"meetingUpdate":{"meetingState":{"isMuted","isVideoOn","isHandRaised","isInMeeting","isBackgroundBlurred",…},"meetingPermissions":{…}}}`.
- Teams nicht gestartet → alle 5 s neu verbinden, Titel „–“.

## Aktionen

| UUID-Suffix | Name | Befehl | Zustand 1 wenn |
| --- | --- | --- | --- |
| `mute` | Mikrofon | `toggle-mute` | `isMuted` |
| `video` | Kamera | `toggle-video` | `isVideoOn` |
| `hand` | Hand heben | `toggle-hand` | `isHandRaised` |
| `blur` | Hintergrund unscharf | `toggle-background-blur` | `isBackgroundBlurred` |
| `react` | Reaktion | `send-reaction` mit `{"type": like/love/applause/laugh/wow}` | – |
| `leave` | Verlassen | `leave-call` | – |

Außerhalb einer Besprechung (`isInMeeting` false) Titel „–“ und `showAlert` beim Drücken.

## Tests

- `meetingUpdate` parsen → Zustände.
- `tokenRefresh` → Speichern ausgelöst.

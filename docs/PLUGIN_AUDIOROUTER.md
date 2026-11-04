# AudioRouter-Plugin

Steuert [AudioRouter](https://audiorouter.org/) vom Deck aus: Play/Stop, Session wechseln, Mikrofon-Privacy, Tools
umschalten, Pegel per Drehregler, Aufnahme und Soundboard. Quellcode: [`plugins/audiorouter`](../plugins/audiorouter).

AudioRouter selbst gibt es nur für Windows. Das Plugin spricht mit dessen lokaler HTTP-API und läuft deshalb auch unter
Linux, wenn AudioRouter die API im lokalen Netz freigibt.

## Einrichten

1. In AudioRouter **API** öffnen und **Start API** wählen. Mit der Startoption der API bleibt sie nach einem Neustart
   bereit.
2. **Copy API token** wählen.
3. **Plugins → Katalog → AudioRouter → Installieren**, dann unter **Einstellungen** das Token einfügen.
   Die Adresse bleibt leer (Standard `http://127.0.0.1:17891`), solange AudioRouter auf demselben PC läuft.
4. Der Status zeigt „Verbunden, Session „…““.

Von einem anderen PC aus: in AudioRouter unter **Who can connect** das lokale Netz wählen und die angezeigte
**Network URL** als Adresse eintragen. Der Verkehr ist dann unverschlüsselt; nur in einem vertrauenswürdigen Netz nutzen.

Wichtig: `127.0.0.1` statt `localhost` eintragen, sonst lehnt AudioRouter mit HTTP 403 ab.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Play/Stop | startet bzw. stoppt die gewählte Session | – | leuchtet, solange sie läuft |
| Mikrofon-Privacy | schaltet die Privacy-Stummschaltung um | – | leuchtet, solange das Mikrofon stumm ist |
| Tool umschalten | schaltet die Einstellung um bzw. hält sie, solange gedrückt | – | leuchtet, solange sie an ist |
| Pegel | ändert den Wert um den Schritt | ändert den Wert um den Schritt je Raste | aktueller Wert, z. B. `60%` |
| Session | wählt die Session | – | Name; leuchtet, solange sie gewählt ist |
| Aufnahme | startet bzw. stoppt die Aufnahme | – | Dauer, z. B. `01:05`; leuchtet während der Aufnahme |
| Sound | spielt den Platz des Soundboards | – | keine |

| Feld | Bemerkung |
| --- | --- |
| Tool | Tools der gewählten Session, z. B. „Mute Discord · mute“ |
| Einstellung (Tool umschalten) | Aktiv (Enabled), Umgangen (Bypass) oder eine Ein/Aus- bzw. A/B-Einstellung des Tools |
| Verhalten | „Umschalten je Druck“ oder „Nur solange gedrückt“ (z. B. Push-to-Talk) |
| Einstellung (Pegel) | Zahl-Einstellung des Tools, z. B. `percent` oder `gainDb` |
| Schritt | pro Raste bzw. Druck, Standard 1; negativ zum Verringern |
| Danach abspielen | „Immer“ startet die neue Session auch, wenn vorher nichts lief |
| Recorder | Recorder-Tool der gewählten Session |
| Soundboard, Platz, Wiedergabe | Soundboard-Tool, Platz 1 bis 9, Wiedergabeart (wie eingestellt, einmal, Schleife, solange gedrückt) |

## Verhalten

- Tasten folgen der in AudioRouter gewählten Session. Eine Session-Taste wechselt sie; alle anderen Tasten folgen mit.
- Beim Session-Wechsel stoppt das Plugin die laufende Session, wählt die neue und startet sie wieder, wenn vorher etwas
  lief. Grund: AudioRouter kann nur eine Session gleichzeitig vorbereiten.
- Der Pegel bleibt im Bereich, den AudioRouter für die Einstellung angibt.
- Aufnehmen geht nur, während die Session läuft; sonst warnt die Taste.
- Das Plugin fragt den Zustand jede Sekunde ab und gleich nach jedem Tastendruck. Änderungen im AudioRouter-Fenster
  erscheinen so nach spätestens einer Sekunde auf dem Deck.
- Ohne Verbindung zeigen alle Tasten `offline`; der Status nennt den Grund (API nicht gestartet, Token ungültig …).
- Fehler wie ein gelöschtes Tool lösen eine Warnung auf der Taste aus; die Meldung von AudioRouter steht im Protokoll.

## Technik

Jede Backend-Methode `a.b` ist `POST /api/v1/a/b` mit `Authorization: Bearer <Token>` und JSON-Körper. Ändernde
Aufrufe bekommen je Druck einen neuen `idempotencyKey`. Genutzt werden:

- Lesen: `sessions.summary`, `sessions.get` (Einstellungen der Tools, nur bei neuer Revision), `nodes.catalog`
  (Wertebereiche und Einheiten), `sessions.list`, `recorders.list`, `status.get`
- Schreiben: `sessions.play`, `session.stop`, `sessions.active.set`, `safety.togglePrivacyMute`, `nodes.toggle`,
  `nodes.set`, `recorders.startRecording`, `recorders.stopRecording`, `soundboard.trigger`

Die API-Doku von AudioRouter: [Local HTTP API](https://github.com/MrDesjardins/audiorouter/blob/main/docs/operations/local-http-api.md).

## Tests

`cargo test -p opendeckn3-audiorouter` prüft die Pfade der Methoden, die Adress- und Token-Prüfung, Ein/Aus- und
A/B-Einstellungen, die Auswahllisten aus dem Katalog, Schritte und Begrenzung, das Format der Werte und die Anzeige mit
und ohne Verbindung. Die Tests sprechen nicht mit einem laufenden AudioRouter.

# YouTube-Plugin

Zeigt vom Deck aus, ob dein YouTube-Livestream läuft, wie viele Zuschauer gerade zusehen, und schickt Nachrichten in
den Live-Chat. Quellcode: [`plugins/youtube`](../plugins/youtube). Windows und Linux.

## Einrichten

Das Plugin braucht einen eigenen OAuth-Client in der Google Cloud. Die **Client-ID** und das **Client-Secret** trägst
du in die Einstellungen des Plugins ein. Google behandelt das Secret bei diesem Client-Typ nicht als vertraulich; es
wird nicht mitgeliefert.

### 1. Google-Cloud-Projekt anlegen

1. Auf [console.cloud.google.com](https://console.cloud.google.com) mit dem Google-Konto anmelden, dem der Kanal gehört.
2. Oben das Projekt-Menü öffnen und **Neues Projekt** wählen, z. B. Name `OpenDeckN3`. Danach dieses Projekt auswählen.

### 2. YouTube Data API v3 aktivieren

1. **APIs & Dienste → Bibliothek** öffnen.
2. **YouTube Data API v3** suchen und **Aktivieren** klicken.

### 3. Zustimmungsbildschirm einrichten

1. **APIs & Dienste → OAuth-Zustimmungsbildschirm** öffnen (in neueren Oberflächen: **Google Auth Platform**).
2. Nutzertyp **Extern** wählen und erstellen.
3. App-Name (z. B. `OpenDeckN3`) und deine E-Mail-Adresse eintragen. Weitere Pflichtfelder sind frei wählbar.
4. Bereiche (Scopes) können leer bleiben.
5. Unter **Testnutzer** deine eigene Google-Adresse eintragen.
6. Der Veröffentlichungsstatus bleibt auf **Testing**. Eine Veröffentlichung ist für die eigene Nutzung nicht nötig.

### 4. OAuth-Client anlegen

1. **APIs & Dienste → Anmeldedaten → Anmeldedaten erstellen → OAuth-Client-ID** wählen.
2. Als Anwendungstyp **Fernseher und Geräte mit eingeschränkter Eingabe** (englisch „TVs and Limited Input devices“)
   wählen. Einen Namen vergeben, z. B. `OpenDeckN3 Deck`.
3. **Erstellen** klicken. Google zeigt **Client-ID** und **Client-Secret** an. Beides kopieren.

### 5. Im Plugin anmelden

1. In OpenDeckN3: **Plugins → Katalog → YouTube Live → Installieren**.
2. **Plugins → YouTube Live → Einstellungen**: Client-ID und Client-Secret eintragen.
3. **Anmelden** klicken. Der Browser öffnet `google.com/device`, und im Feld **Status** erscheint ein Code
   (z. B. `ABCD-EFGH`).
4. Den Code auf der Google-Seite eingeben, mit dem Konto des Kanals anmelden und den Zugriff erlauben.
5. Der Status zeigt „Verbunden“. Die Anmeldung wird gespeichert und vor Ablauf automatisch erneuert.

Mit **Abmelden** werden die gespeicherten Anmeldedaten gelöscht. Ändert man Client-ID oder Client-Secret, meldet sich
das Plugin ebenfalls ab. Zeigt der Status „Bitte neu anmelden“, hat Google den Zugriff widerrufen: erneut **Anmelden**
klicken.

Hinweis: Solange der Zustimmungsbildschirm im Modus **Testing** ist, läuft die Anmeldung nach etwa 7 Tagen ab. Dann
zeigt der Status „Bitte neu anmelden“; einfach erneut **Anmelden** klicken.

## Aktionen

| Aktion | Taste | Anzeige | Verhalten |
| --- | --- | --- | --- |
| Live-Status | Taste öffnet YouTube Studio | leuchtet und zeigt `LIVE`, solange ein Livestream läuft; sonst `offline` | – |
| Zuschauer | – | Zuschauerzahl (`1,2 Tsd.` ab 1000) oder `offline` | – |
| Chat-Nachricht | sendet die eingestellte Nachricht in den Live-Chat | – | Nachricht in den Einstellungen, höchstens 200 Zeichen |

Die Chat-Nachricht läuft über den aktiven Livestream. Ist gerade keiner aktiv, zeigt die Taste einen Fehler und der
Grund steht im Log.

Der Live-Status und die Zuschauerzahl werden nur abgefragt, solange eine Status- oder Zuschauer-Taste sichtbar ist,
und höchstens alle 60 Sekunden; beide Tasten teilen sich eine Abfrage.

## Kontingent

Die YouTube Data API hat ein Tageskontingent von 10 000 Einheiten pro Google-Projekt. Ungefähre Kosten:

- Live-Status abfragen: 1 Einheit pro Minute, dazu 1 Einheit für die Zuschauerzahl, solange ein Stream läuft.
- Chat-Nachricht senden: 50 Einheiten pro Nachricht.

Ist das Kontingent erschöpft, zeigt der Status „YouTube-Kontingent erschöpft“, und das Plugin fragt eine Stunde lang
nichts ab. Das Kontingent gilt für das ganze Projekt, nicht nur für dieses Plugin.

## Technik

- Anmeldung mit dem Device Flow (`oauth2.googleapis.com/device/code` und `/token`) für Geräte mit eingeschränkter Eingabe.
- Berechtigung: `https://www.googleapis.com/auth/youtube`.
- Die Anmeldedaten liegen nur in den globalen Einstellungen des Plugins (`accessToken`, `refreshToken`, `expiresAt`).
  Sie werden nie ins Log geschrieben. Erneuert wird kurz vor Ablauf und einmal nach einer Antwort 401.
- API: `googleapis.com/youtube/v3`: `GET liveBroadcasts` (mit `broadcastStatus=active&broadcastType=all`),
  `GET videos?part=liveStreamingDetails`, `POST liveChat/messages`.
- Tests: `cargo test -p opendeckn3-youtube` (Gerätecode-Antworten, Token-Erneuerung, aktiver Stream, Zuschauerzahl,
  Kontingentfehler). Es gibt keine Tests gegen echte Google-Dienste.

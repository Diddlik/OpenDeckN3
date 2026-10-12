# Twitch-Plugin

Steuert deinen Twitch-Kanal vom Deck aus: Marker setzen, Clips erstellen, Chat-Modi umschalten, Werbung starten und
die Zuschauerzahl anzeigen. Quellcode: [`plugins/twitch`](../plugins/twitch). Windows und Linux.

## Einrichten

Das Plugin braucht eine eigene Twitch-Anwendung, erkennbar an der **Client-ID**. Ein Client-Secret ist nicht nötig und
wird nicht mitgeliefert.

### 1. Anwendung bei Twitch registrieren

1. Auf [dev.twitch.tv/console](https://dev.twitch.tv/console) mit dem Twitch-Konto anmelden, dem der Kanal gehört.
   Twitch verlangt für die Entwicklerkonsole in der Regel eine aktive Zwei-Faktor-Authentifizierung.
2. **Anwendung registrieren** klicken.
3. Die Felder ausfüllen:
   - **Name:** frei wählbar, muss bei Twitch eindeutig sein, z. B. `OpenDeckN3 Deck`.
   - **OAuth-Weiterleitungs-URLs:** `http://localhost` eintragen und mit **Hinzufügen** übernehmen. Der Gerätecode-Ablauf
     nutzt die Adresse nicht, Twitch verlangt aber einen Eintrag.
   - **Kategorie:** beliebig, z. B. „Sonstige“ (in der englischen Oberfläche „Other“).
   - **Client-Typ:** **Public**. Nicht „Confidential“ wählen, sonst verlangt Twitch ein Secret.
4. **Erstellen** klicken. Die Anwendung erscheint in der Liste; über **Verwalten** öffnen.
5. Die **Client-ID** kopieren. Ein Client-Secret wird nicht gebraucht und muss nicht erzeugt werden.

### 2. Im Plugin anmelden

1. In OpenDeckN3: **Plugins → Katalog → Twitch → Installieren**.
2. **Plugins → Twitch → Einstellungen**: die Client-ID in das Feld **Client-ID** einfügen.
3. **Anmelden** klicken. Der Browser öffnet `twitch.tv/activate`, und im Feld **Status** erscheint ein Code
   (z. B. `ABCD-EFGH`).
4. Den Code auf der Twitch-Seite eingeben, mit dem Konto des Kanals anmelden und den Zugriff erlauben.
5. Der Status zeigt „Verbunden als …“. Die Anmeldung bleibt gespeichert und wird vor Ablauf automatisch erneuert.

Mit **Abmelden** in den Einstellungen werden die gespeicherten Anmeldedaten gelöscht. Ändert man die Client-ID,
meldet sich das Plugin ebenfalls ab. Zeigt der Status „Bitte neu anmelden“, hat Twitch den Zugriff widerrufen
(z. B. nach einer Passwortänderung): erneut **Anmelden** klicken.

## Aktionen

| Aktion | Taste | Anzeige | Verhalten |
| --- | --- | --- | --- |
| Marker setzen | setzt einen Marker im laufenden Stream | – | optionale Beschreibung, höchstens 140 Zeichen |
| Clip erstellen | erstellt einen Clip aus dem laufenden Stream | – | mit „Bearbeiten-Link öffnen: Ja“ öffnet sich der Link zum Bearbeiten |
| Chat-Modus | schaltet den gewählten Modus um | leuchtet, solange der Modus an ist; Name als Titel | Langsam, Follower, Abo, Nur Emotes oder Einzigartige Nachrichten |
| Zuschauer | öffnet deinen Kanal auf twitch.tv | Zuschauerzahl (`1,2 Tsd.` ab 1000) oder `offline` | – |
| Werbung | startet eine Werbepause | – | 30, 60, 90, 120 oder 180 Sekunden |

Schlägt eine Aktion fehl (etwa Werbung ohne laufenden Stream), zeigt die Taste einen Fehler und der Grund steht im Log.

Die Zuschauerzahl wird nur abgefragt, solange eine Zuschauer-Taste sichtbar ist, und höchstens alle 60 Sekunden;
alle Zuschauer-Tasten teilen sich eine Anfrage. Der Zustand der Chat-Modi wird beim Erscheinen der Taste und nach
jedem Umschalten gelesen.

## Technik

- Anmeldung mit dem Device Code Grant (`id.twitch.tv/oauth2/device` und `/oauth2/token`), ohne Client-Secret.
- Berechtigungen: `channel:manage:broadcast clips:edit moderator:manage:chat_settings channel:edit:commercial`.
- Die Anmeldedaten liegen nur in den globalen Einstellungen des Plugins (`accessToken`, `refreshToken`, `expiresAt`,
  `userId`, `login`). Sie werden nie ins Log geschrieben. Erneuert wird kurz vor Ablauf und einmal nach einer
  Antwort 401.
- API: Helix unter `api.twitch.tv/helix`: `POST /streams/markers`, `POST /clips`, `GET` und `PATCH /chat/settings`,
  `GET /streams`, `POST /channels/commercial`, `GET /users`.
- Tests: `cargo test -p opendeckn3-twitch` (Token-Ablauf, Antworten des Gerätecode-Ablaufs, Zuschauerzahl,
  Chat-Modus). Es gibt keine Tests gegen echtes Twitch.

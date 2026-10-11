# Mail-&-Kalender-Plugin

Zeigt **ungelesene Mails** und den **nächsten Termin** auf dem Deck. Ohne Google-API und ohne Anmeldung per
Google-Konto: Mails kommen über **IMAP** (App-Passwort), Termine über die **geheime iCal-Adresse** eines Kalenders.
Das funktioniert mit Gmail und Google Kalender ebenso wie mit Outlook, iCloud, Nextcloud oder jedem anderen
Anbieter. Quellcode: [`plugins/mailcalendar`](../plugins/mailcalendar).

## Einrichten

1. **Plugins → Katalog → Mail & Kalender → Installieren.**
2. Unter **Plugins → Mail & Kalender → Einstellungen**:
   - **Kalender (iCal-Adressen)**: eine Zeile pro Kalender, `Name = Adresse`.
     - *Google Kalender*: [calendar.google.com](https://calendar.google.com) → Zahnrad → Einstellungen → links den
       Kalender wählen → **Geheime Adresse im iCal-Format** kopieren.
     - *Outlook/Microsoft 365*: Einstellungen → Kalender → Freigegebene Kalender → **Kalender veröffentlichen** →
       ICS-Link kopieren.
     - *iCloud*: Kalender freigeben → **Öffentlicher Kalender** → Link (`webcal://…`) kopieren.
   - **IMAP-Server**, z. B. `imap.gmail.com`, `outlook.office365.com`, `imap.gmx.net`. Verbunden wird verschlüsselt
     über Port 993; ein anderer Port als `server:port`.
   - **E-Mail-Adresse** und **App-Passwort**. *Gmail*: Google-Konto → Sicherheit → Bestätigung in zwei Schritten
     einschalten → [App-Passwörter](https://myaccount.google.com/apppasswords) → neues Passwort erstellen. Andere
     Anbieter: Mail-Passwort oder deren App-Passwort.
3. **Verbindung prüfen** – der Status zeigt z. B. „Kalender: 2 geladen · Mail: verbunden mit imap.gmail.com“.

Die geheime iCal-Adresse und das App-Passwort geben Lesezugriff auf Kalender bzw. Postfach. Sie liegen in den
Plugin-Einstellungen im Konfigurationsordner und werden nicht in Profil-Exporte geschrieben.

## Aktionen

| Aktion | Anzeige | Taste |
| --- | --- | --- |
| Ungelesene Mails | Anzahl ungelesener Mails im gewählten Ordner (`INBOX` oder ein Gmail-Label); rot bei neuen Mails, leer bei 0 | öffnet das Postfach (einstellbar, Standard Gmail) |
| Nächster Termin | `in 12 min`, `14:30`, `Mo 09:00` oder `jetzt`, im Wechsel mit dem Titel; gelb ab einstellbarer Vorlaufzeit (Standard 5 min) und während des Termins; `frei`, wenn in den nächsten 8 Tagen nichts ansteht | öffnet den Meeting-Link des Termins (Meet, Teams, Zoom, Webex, Whereby), sonst den Kalender |

`offline` auf der Taste heißt: Zugang fehlt oder der Server ist nicht erreichbar – Details im Status.

## Grenzen

- Mails werden jede Minute geprüft, Kalender alle 5 Minuten neu geladen. Google aktualisiert die iCal-Adresse selbst
  mit etwas Verzögerung; frisch angelegte Termine können daher einige Minuten später erscheinen.
- Ganztägige Termine werden nicht angezeigt.
- Nur ein Mail-Konto; Ordnernamen mit Umlauten werden noch nicht unterstützt.
- Server, die nur STARTTLS auf Port 143 anbieten, werden nicht unterstützt.

## Technik

IMAP über TLS: `LOGIN`, `STATUS <ordner> (UNSEEN)`, `LOGOUT` pro Prüfung. Kalender: HTTP-GET der Adresse
(`webcal://` wird zu `https://`), eigener iCalendar-Leser mit Wiederholungen (`RRULE`, `RDATE`, `EXDATE`, über das
Crate `rrule`), verschobenen oder abgesagten Einzelterminen (`RECURRENCE-ID`, `STATUS:CANCELLED`) und Zeitzonen
(`TZID`; unbekannte Namen, z. B. Windows-Zeitzonen aus Outlook, gelten als lokale Zeit). Tests ohne Server:
`cargo test -p opendeckn3-mailcalendar` (simulierter IMAP- und HTTP-Server).

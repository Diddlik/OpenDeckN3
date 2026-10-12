# GitHub-Plugin

Zeigt vom Deck aus, ob der letzte Workflow-Lauf eines Branches läuft, erfolgreich war oder fehlgeschlagen ist, wie viele
Pull Requests auf dich warten und wie viele Benachrichtigungen ungelesen sind. Quellcode:
[`plugins/github`](../plugins/github). Windows und Linux.

## Einrichten

Ein Token ist optional. Ohne Token funktionieren **öffentliche Repos** für CI-Status und Pull Requests; GitHub erlaubt
dann aber nur 60 Abfragen pro Stunde. Benachrichtigungen brauchen immer ein Token.

Das Token trägst du unter **Plugins → GitHub → Einstellungen** im Feld **Token** ein. Der Status zeigt, ob ein Token
gesetzt ist, oder meldet „Token ungültig“, wenn GitHub es ablehnt.

### Feinkörniges Token (für CI-Status und Pull Requests)

1. Auf github.com unter **Settings → Developer settings → Personal access tokens → Fine-grained tokens** öffnen und
   **Generate new token** wählen.
2. **Repository access** wählen: **Only select repositories** und die gewünschten Repos ankreuzen, oder **All
   repositories**.
3. Unter **Repository permissions** nur lesend freigeben:
   - **Actions**: Read-only
   - **Pull requests**: Read-only
   - **Metadata**: Read-only (wird automatisch gesetzt)
4. Das Token erstellen und in das Feld **Token** kopieren. GitHub zeigt es nur einmal.

Ein feinkörniges Token sieht nur die Repos, die du ausgewählt hast. Pull-Request-Zähler zählen nur Repos, auf die das
Token Zugriff hat.

### Klassisches Token (für Benachrichtigungen)

Benachrichtigungen lassen sich mit einem feinkörnigen Token nicht abrufen. Dafür brauchst du ein **klassisches Token**:

1. **Settings → Developer settings → Personal access tokens → Tokens (classic)** öffnen und **Generate new token**
   wählen.
2. Den Bereich **`notifications`** ankreuzen. Für private Repos außerdem **`repo`**, weil klassische Tokens dort sonst
   keine Läufe und Pull Requests zeigen.
3. Das Token erstellen und in das Feld **Token** eintragen.

Ein klassisches Token gilt für alle Repos des Kontos und hat mehr Rechte als nötig. Wer nur CI-Status und Pull Requests
braucht, nimmt das feinkörnige Token.

## Aktionen

| Aktion | Taste | Anzeige | Taste öffnet |
| --- | --- | --- | --- |
| CI-Status | neuester Lauf des Branches | Name des Workflows; grün = erfolgreich, gelb = läuft oder wartet, rot = fehlgeschlagen | den Lauf auf github.com, sonst die Actions-Seite des Repos |
| Pull Requests | Anzahl offener Pull Requests | Anzahl | Suche auf github.com |
| Benachrichtigungen | Anzahl ungelesener Benachrichtigungen (höchstens 50) | Anzahl; leuchtet, wenn mindestens eine da ist | github.com/notifications |

### CI-Status

- **Repository**: `Besitzer/Name`, z. B. `Diddlik/OpenDeckN3`.
- **Branch**: standardmäßig `main`.
- **Workflow-Datei** (optional): z. B. `ci.yml`. Ohne Angabe zählt der neueste Lauf aller Workflows des Branches.

Als fehlgeschlagen gelten die Ergebnisse `failure`, `cancelled`, `timed_out`, `startup_failure` und `action_required`.
Läufe in der Warteschlange oder in Arbeit zeigen den gelben Zustand.

### Pull Requests

Der Filter legt fest, welche Pull Requests gezählt werden:

- **Ich soll prüfen**: Pull Requests, bei denen deine Review angefragt ist (`review-requested:@me`).
- **Von mir**: Pull Requests, die du geöffnet hast (`author:@me`).
- **Eines Repos**: alle offenen Pull Requests eines Repos; das Repo trägst du zusätzlich ein.

## Abfragen und Grenzen

- Jede Anfrage wird höchstens einmal pro Minute ausgeführt. Mehrere Tasten mit derselben Anfrage teilen sich das
  Ergebnis. Nur sichtbare Tasten werden abgefragt.
- GitHub begrenzt die Anzahl der Anfragen: 60 pro Stunde ohne Token, 5000 mit Token. Die Suche (Pull Requests) hat ein
  eigenes Limit von 10 Anfragen pro Minute ohne Token und 30 mit Token.
- Ist das Limit aufgebraucht, pausiert das Plugin bis zum Zurücksetzen des Limits und zeigt das im Status.
- Ein Fehler (z. B. unbekanntes Repo, Branch oder Workflow) zeigt `–` auf der Taste. Die Ursache steht im Protokoll
  der App.
- Das Token wird nur in den globalen Einstellungen gespeichert und nicht protokolliert.

Tests: `cargo test -p opendeckn3-github`.

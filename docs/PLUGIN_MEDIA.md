# Medien-Plugin

Steuert die gerade laufende Wiedergabe (Spotify, Browser, VLC …) vom Deck aus und zeigt Titel und Cover auf der
Taste. Die eingebaute Aktion „Medien“ drückt nur Medientasten; dieses Plugin kennt zusätzlich den Zustand.
Quellcode: [`plugins/media`](../plugins/media). Windows und Linux.

## Einrichten

Es gibt keine globalen Einstellungen und keine Anmeldung. Das Plugin liest den Zustand des Systems:

- **Windows:** die Medienwiedergabe der Windows-Medienübersicht (System Media Transport Controls). Gemeint ist die
  Sitzung, die das System gerade als aktuell führt.
- **Linux:** MPRIS über D-Bus. Gemeint ist der erste Player `org.mpris.MediaPlayer2.*`, der gerade spielt; sonst der
  erste gefundene.

Der Player muss die Wiedergabe über dieses System melden. Die meisten Player tun das von selbst (Spotify, Browser,
VLC).

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Wiedergabe/Pause | startet oder pausiert | – | Symbol für Wiedergabe oder Pause; optional das Cover |
| Nächster Titel | springt weiter | – | Symbol |
| Vorheriger Titel | springt zurück | – | Symbol |
| Läuft gerade | startet oder pausiert | spult oder wechselt den Titel | Cover und Titel (gekürzt auf 14 Zeichen) |

### Wiedergabe/Pause

| Feld | Bemerkung |
| --- | --- |
| Cover anzeigen | Nein (Standard) oder Ja. Mit Cover ersetzt das Bild die Symbole für Wiedergabe und Pause |

### Läuft gerade

| Feld | Bemerkung |
| --- | --- |
| Drehregler | „Spulen“ (Standard): 5 Sekunden je Rastung. „Titel wechseln“: eine Rastung ist ein nächster bzw. vorheriger Titel |

Drücken des Drehreglers startet oder pausiert die Wiedergabe.

## Anzeige

- Ist nichts abspielbar, zeigt „Läuft gerade“ als Titel `–`.
- Das Cover wird auf 144 × 144 Pixel zugeschnitten. Es wird nur neu gesendet, wenn sich das Cover-Bild ändert.
- Unter Linux wird nur ein lokales Cover (`file://`) geladen. Cover aus dem Netz, etwa bei Spotify, bleiben leer und
  das Standardbild erscheint.
- Kann das System nicht abgefragt werden, zeigt die Taste `–` als Titel. Der Grund steht einmal im Log des Dienstes,
  bis sich der Fehler ändert. Ein Befehl, der scheitert (etwa ohne Player), zeigt eine Warnung auf der Taste.

## Tests

`cargo test -p opendeckn3-media` prüft das Kürzen des Titels, den Vergleich von Covern über ihren Hash, die
Umwandlung des Covers in ein 144 × 144-PNG und unter Linux die Auswahl des Players. Die Tests sprechen nicht mit
einem echten Player.

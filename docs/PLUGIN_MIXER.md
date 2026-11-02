# Lautstärke-Plugin (pro App)

Ein Drehregler pro Anwendung, wie ein Mischpult: Spiel, Discord oder Browser werden leiser oder lauter, ohne den
Windows-Mixer zu öffnen. Quellcode: [`plugins/mixer`](../plugins/mixer). Windows und Linux.

## Einrichten

Es gibt keine globalen Einstellungen und keine Anmeldung. Das Plugin liest die Audio-Sitzungen des Systems:

- **Windows:** WASAPI-Sitzungen des Standard-Ausgabegeräts. Der Prozessname (z. B. `Discord.exe`) ordnet eine Sitzung
  einer Anwendung zu. Systemtöne werden nicht angeboten.
- **Linux:** PulseAudio bzw. PipeWire (mit `pipewire-pulse`) über `pactl`. Zugeordnet wird über
  `application.process.binary`, sonst über den Anwendungsnamen. `pactl` muss installiert sein.

Die Anwendung muss gerade Ton ausgeben oder eine Audio-Sitzung offen haben, damit sie in der Auswahl erscheint.
Freitext ist ebenfalls möglich; Groß-/Kleinschreibung und `.exe` spielen keine Rolle.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| App-Lautstärke | stumm umschalten | Lautstärke ändern | Prozent bzw. „stumm“; Zustand 1 = stumm |
| Aktive App | stumm umschalten | Lautstärke ändern | wie oben, für die App im Vordergrund (nur Windows) |

| Feld | Bemerkung |
| --- | --- |
| Anwendung | nur bei „App-Lautstärke“: Prozessname aus der Liste oder Freitext |
| Schritt | Prozent je Raste, Standard 2, Bereich 1 bis 20 (nur Drehregler) |

## Verhalten

- Drehen ändert die Lautstärke aller Sitzungen dieser Anwendung, begrenzt auf 0 bis 100 %.
- Drücken oder Drücken des Drehreglers schaltet alle Sitzungen dieser Anwendung gleichzeitig stumm oder laut.
- Die Anzeige wird jede Sekunde aktualisiert. Änderungen im Windows-Mixer erscheinen also mit höchstens einer Sekunde
  Verzögerung.
- Läuft die Anwendung nicht oder hat sie keine Audio-Sitzung, zeigt die Taste `–`.
- „Aktive App“ zeigt `–`, wenn kein Fenster im Vordergrund steht oder dessen Anwendung keine Audio-Sitzung hat.
  Unter Linux gibt es kein Vordergrundfenster in `pactl`, dort bleibt die Aktion ohne Funktion.

## Tests

`cargo test -p opendeckn3-mixer` prüft den Vergleich von Prozessnamen, die Auswahlliste ohne Dubletten, die Begrenzung
der Lautstärke, die Anzeige von Prozent und „stumm“ sowie das Parsen der `pactl`-Ausgabe (Beispiel im Test). Die
Tests sprechen nicht mit echten Audio-Sitzungen.

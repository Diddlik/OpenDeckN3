# VoiceMeeter-Plugin

Steuert [VoiceMeeter](https://vb-audio.com/Voicemeeter/) (Standard, Banana und Potato) vom Deck aus: Pegel per
Drehregler, Stummschaltung, Bus-Routing und eigene Befehle. Quellcode: [`plugins/voicemeeter`](../plugins/voicemeeter).
Nur Windows.

## Einrichten

1. VoiceMeeter installieren und starten. Das Plugin spricht mit dem laufenden Programm.
2. **Plugins → Katalog → VoiceMeeter → Installieren.** Der Status zeigt „Verbunden mit VoiceMeeter Banana“ (bzw.
   Standard oder Potato).

Das Plugin lädt `VoicemeeterRemote64.dll` aus dem Installationsordner. Den Ordner liest es aus der Registry; fehlt der
Eintrag, nimmt es `C:\Program Files (x86)\VB\Voicemeeter`. Läuft VoiceMeeter nicht, zeigt der Status
„VoiceMeeter läuft nicht“. Das Plugin versucht dann alle 5 Sekunden erneut, sich zu verbinden.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Pegel | stumm umschalten | Pegel ändern | Pegel in dB, z. B. `-6,0 dB`; Zustand 1 = stumm |
| Stumm | stumm umschalten | – | leuchtet, solange der Kanal stumm ist |
| Routing | Verbindung ein- bzw. ausschalten | – | leuchtet, solange die Verbindung besteht |
| Befehl | führt den Text als Parameterbefehl aus | – | keine |

| Feld | Bemerkung |
| --- | --- |
| Kanal | Strip (Eingang) oder Bus (Ausgang), Auswahl aus der laufenden VoiceMeeter-Version |
| Schritt | dB je Raste, Standard 1, Bereich 0,5 bis 12 (nur Drehregler) |
| Eingang / Ausgang | Strip und Bus für „Routing“ |
| Befehl | Text in der Syntax der Remote-API, z. B. `Strip[0].Mute=1; Bus[0].Gain=-10;` |

## Verhalten

- Drehen ändert den Pegel um den Schritt je Raste. Der Pegel bleibt zwischen −60 und +12 dB.
- Drücken des Pegel-Reglers oder der Taste schaltet den Kanal stumm bzw. laut.
- Ausgänge je Version: Standard A1 und B1; Banana A1 bis A3 und B1 bis B2; Potato A1 bis A5 und B1 bis B3.
- Änderungen im VoiceMeeter-Fenster erscheinen auf dem Deck. Das Plugin prüft alle 100 ms, ob sich etwas geändert
  hat, und liest dann die Werte neu.
- Ein Fehler (etwa ein unbekannter Parameter im Befehl) löst eine Warnung auf der Taste aus; die Details stehen im
  Protokoll.
- Ist VoiceMeeter nicht verbunden, zeigen Pegel-Tasten `–`, die anderen Tasten bleiben dunkel. Ein Druck meldet einen
  Fehler.

## Technik

Das Plugin nutzt die offizielle Remote-API aus `VoicemeeterRemote64.dll`:

- `VBVMR_Login`, `VBVMR_Logout`, `VBVMR_GetVoicemeeterType`, `VBVMR_IsParametersDirty`
- `VBVMR_GetParameterFloat`, `VBVMR_SetParameterFloat`, `VBVMR_SetParameters`

Typen: 1 Standard (3 Strips, 2 Busse), 2 Banana (5 Strips, 5 Busse), 3 und 6 Potato (8 Strips, 8 Busse).
Parameternamen: `Strip[n].Gain`, `Strip[n].Mute`, `Strip[n].A1` (Routing), `Bus[n].Gain`, `Bus[n].Mute`.

Alle Aufrufe der DLL laufen in einem eigenen Thread. Die übrigen Teile des Plugins sprechen ihn über Kanäle an.

## Tests

`cargo test -p opendeckn3-voicemeeter` prüft die Strip- und Busanzahl je Version, die Parameternamen, die
Auswahllisten, die dB-Begrenzung und die Anzeige mit und ohne Verbindung. Die Auswertung des Registry-Werts
(Installationsordner) prüft ein Test an Beispieltexten. Die Tests sprechen nicht mit einem laufenden VoiceMeeter.

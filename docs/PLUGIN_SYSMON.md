# Systemmonitor-Plugin

Zeigt Werte des Rechners als Verlaufsgrafik auf der Taste: Auslastung von CPU, Arbeitsspeicher und GPU, Temperaturen,
Netzwerkdurchsatz und Belegung der Laufwerke. Quellcode: [`plugins/sysmon`](../plugins/sysmon). Windows und Linux.

## Einrichten

Es gibt keine globalen Einstellungen und keine Anmeldung. Alle Werte kommen vom System selbst.

**GPU (nur NVIDIA):** Die Werte liest das Plugin über NVML aus dem NVIDIA-Treiber (`nvml.dll` unter Windows,
`libnvidia-ml.so` unter Linux). Fehlt der Treiber oder keine NVIDIA-Karte, zeigen die GPU-Aktionen `–` und das Log des
Dienstes enthält einmal einen Hinweis. Andere Grafikkarten (AMD, Intel) werden noch nicht unterstützt.

**Temperaturen:** Windows liefert über das Plugin oft keine CPU-Temperatur. Dann zeigt die Taste `–`. Unter Linux
kommen sie aus den Sensoren im Kernel (etwa `coretemp` oder `k10temp`).

## Aktion

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Systemwert | öffnet den Task-Manager (nur Windows) | – | Verlaufsgrafik oder nur Zahl; der aktuelle Wert steht als Titel auf der Taste |

| Feld | Bemerkung |
| --- | --- |
| Wert | CPU-Auslastung, Arbeitsspeicher, GPU-Auslastung, GPU-Speicher, GPU-Temperatur, CPU-Temperatur, Netzwerk Empfang, Netzwerk Senden oder Laufwerk belegt |
| Laufwerk | nur bei „Laufwerk belegt“: das Laufwerk aus der Liste (Einbindepunkt, z. B. `C:\` oder `/`) |
| Anzeige | Verlaufsgrafik (Standard) oder Nur Zahl |
| Linienfarbe | Farbe als `#rrggbb`. Leer: Standardfarbe des Werts |

## Anzeige

- **Verlaufsgrafik:** 144 × 144 Pixel, dunkler Hintergrund, gefüllte Fläche der letzten 60 Sekunden. Die Kopfzeile
  nennt den Wert (CPU, RAM, GPU, VRAM, GPU TEMP, CPU TEMP, NET DOWN, NET UP, DISK).
- **Nur Zahl:** das Standardbild der Aktion, nur der Titel zeigt den Wert.
- Werte: Prozent (`42 %`), Temperatur (`61 °C`), Datenrate (`3,2 MB/s`, `512 KB/s`, `0 B/s`).
- Netzwerk: Summe aller Schnittstellen, Rate pro Sekunde. Die Skala der Grafik passt sich dem größten Wert der letzten
  60 Sekunden an.
- Laufwerk: belegter Anteil in Prozent.
- Ist ein Wert nicht verfügbar, zeigt die Taste `–`.

Die Werte werden jede Sekunde abgefragt, aber nur, solange mindestens eine Systemwert-Taste auf dem Deck sichtbar ist.

## Tasten

Drücken öffnet unter Windows den Task-Manager (`taskmgr.exe`). Unter Linux passiert nichts.

## Tests

`cargo test -p opendeckn3-sysmon` prüft den Verlaufspuffer mit 60 Werten, die Formatierung der Werte mit deutschem
Dezimalkomma, die Farbangabe, die Auswahl der CPU-Sensoren und die Erzeugung der 144 × 144-Grafik. Die Tests lesen keine
echten Messwerte.

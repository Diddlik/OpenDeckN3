# 03 – Systemmonitor

Live-Werte als Verlaufsgrafik auf der Taste.

- UUID `de.opendeckn3.sysmon`, Ordner `plugins/sysmon`, Win + Linux.
- Abhängigkeiten: `sysinfo` (CPU, RAM, Netz, Laufwerke, Temperaturen), `image` + `base64` (Workspace) fürs Zeichnen.
- GPU: `nvml-wrapper` (lädt `nvml.dll`/`libnvidia-ml.so` zur Laufzeit). Fehlt der Treiber, zeigt die GPU-Aktion „–“
  und loggt einmal. AMD/Intel-GPU: nicht im ersten Schritt.

## Aktion `de.opendeckn3.sysmon.metric` – „Systemwert“ (Keypad)

| Feld | Typ | Bemerkung |
| --- | --- | --- |
| `metric` | select | `cpu`, `ram`, `gpu`, `vram`, `gputemp`, `cputemp`, `netdown`, `netup`, `disk` |
| `disk` | select (`source: "disks"`) | nur bei `disk` (`showIf`) |
| `style` | select | `graph` (Standard), `number` |
| `color` | text | Linienfarbe als `#rrggbb`, Standard je Metrik |

- Abfrage jede Sekunde, nur solange eine Instanz sichtbar ist.
- `graph`: 144×144 PNG, dunkler Hintergrund, gefüllte Fläche der letzten 60 Werte, Kopfzeile mit Kürzel (CPU, RAM …),
  aktueller Wert als Titel (`42 %`, `61 °C`, `3,2 MB/s`).
- `number`: nur Titel, Standardbild aus dem Manifest.
- Netz: Summe aller Schnittstellen, Rate aus Differenz pro Sekunde, Skala automatisch (Maximum der letzten 60 Werte).
- Temperaturen: auf Windows liefert `sysinfo` oft nichts → „–“, in der Doku erklären.
- Taste drücken öffnet den Task-Manager (`taskmgr.exe`) bzw. unter Linux nichts.

## Tests

- Ringpuffer mit 60 Werten.
- Formatierung: Prozent, Temperatur, Bytes/s mit deutschem Dezimalkomma.
- Zeichenfunktion: Bildgröße 144×144, Wert 0 und 100 ohne Panik.

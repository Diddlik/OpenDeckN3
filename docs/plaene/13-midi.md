# 13 – MIDI

Das N3 als MIDI-Controller für DAWs (Ableton, Reaper, Bitwig) und über MIDI2LR auch für Lightroom Classic.

- UUID `de.opendeckn3.midi`, Ordner `plugins/midi`, Win + Linux.
- Abhängigkeit: `midir`. Linux braucht zum Bauen `libasound2-dev` → in `AGENTS.md` (Umgebung) und in der
  Linux-CI (`.github/workflows/ci.yml`, apt-Schritt) ergänzen.
- Ausgang: global `output` (select, `source: "outputs"`). Windows kennt keine virtuellen Ports → Doku empfiehlt
  loopMIDI; Linux: zusätzlich Option „Virtueller Port OpenDeckN3“ (`create_virtual`).
- Eingang (optional): global `input` für Rückmeldungen der DAW (Tasten leuchten).

## Aktionen

| UUID-Suffix | Name | Controller | Einstellungen | Verhalten |
| --- | --- | --- | --- | --- |
| `note` | Note | Keypad | `channel` 1–16, `note` 0–127, `velocity` (127), `mode` `momentary`/`toggle` | `keyDown` → Note On, `keyUp` → Note Off (bzw. Umschalten) |
| `cc` | Controller (CC) | Encoder, Keypad | `channel`, `cc`, `mode` `absolute`/`relative` (Zweierkomplement 1/127), `step` | Drehen: Wert ändern/relativ senden; Taste: 0 ↔ 127 |
| `program` | Programmwechsel | Keypad | `channel`, `program` | Program Change senden |

Rückmeldung: eingehende Note On/CC mit gleichem Kanal und Nummer setzt Zustand 1 bei Wert > 0.

## Tests

- MIDI-Bytes bauen (Status-Byte mit Kanal, Klemmen auf 0–127).
- Relativer Modus: +3 → `3`, −2 → `126`.
- Eingehende Nachricht der richtigen Taste zuordnen.

## Doku

`docs/PLUGIN_MIDI.md` mit loopMIDI-Einrichtung und Kurzanleitung für MIDI2LR (Lightroom).

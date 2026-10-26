# AudioSwitcher-Plugin

Schaltet die Audioprofile von [AudioSwitcher](https://github.com/Diddlik/AudioSwitcher) vom Deck aus: ein Druck setzt
Ausgabe- und Eingabegerät eines Profils als Windows-Standard. Außerdem lässt sich das Mikrofon stummschalten. Quellcode: [`plugins/audioswitcher`](../plugins/audioswitcher).
Nur Windows.

## Einrichten

1. AudioSwitcher installieren, Profile anlegen und speichern. AudioSwitcher muss laufen (Tray genügt) und die
   Fernsteuerung mitbringen (Version mit `AudioSwitcher.Control`-Schnittstelle, siehe unten).
2. **Plugins → Katalog → AudioSwitcher → Installieren.** Der Status zeigt „Verbunden mit AudioSwitcher – N Profile“.

## Aktionen

| Aktion | Taste | Drehregler | Anzeige |
| --- | --- | --- | --- |
| Audioprofil | aktiviert das gewählte Profil | – | leuchtet, solange das Profil aktiv ist; Profilname optional als Titel |
| Profile umschalten | wechselt zwischen den zwei Schnellwechsel-Profilen aus AudioSwitcher | blättert durch alle verfügbaren Profile, Drücken schaltet um | Name des aktiven Profils |
| Mikrofon stumm | schaltet das Standardmikrofon stumm oder wieder ein | – | leuchtet rot, solange das Mikrofon stumm ist – auch wenn es anders stummgeschaltet wurde |

Aktiv ist ein Profil, wenn seine Geräte gerade die Windows-Standardgeräte sind – auch wenn sie anders als über
AudioSwitcher gesetzt wurden. Der Zustand wird alle 2 Sekunden abgefragt. `offline` heißt: AudioSwitcher läuft nicht
oder ist zu alt; `fehlt` heißt: das gewählte Profil gibt es nicht mehr. Die Mikrofontaste zeigt `zu alt`, wenn
AudioSwitcher vor Version 1.7.0 läuft, und `kein Mikro`, wenn Windows kein Standardmikrofon hat.

## Technik

AudioSwitcher bietet die Named Pipe `\.\pipe\AudioSwitcher.Control` an, nur für den angemeldeten Benutzer. Pro Zeile
eine JSON-Anfrage, pro Anfrage eine JSON-Zeile als Antwort:

```json
{ "command": "state" }
{ "command": "activate", "profileId": "<guid>" }
{ "command": "toggle" }
{ "command": "mute" }
```

```json
{ "ok": true, "error": null, "activeProfileId": "<guid>", "microphoneMuted": false,
  "profiles": [{ "id": "<guid>", "name": "Headset", "available": true }] }
```

Jede Antwort enthält den aktuellen Zustand; bei `ok: false` steht in `error` der Grund (in der Sprache von
AudioSwitcher). `microphoneMuted` ist `null` ohne Standardmikrofon und fehlt vor AudioSwitcher 1.7.0. Tests: `cargo test -p opendeckn3-audioswitcher`.

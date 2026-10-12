# 05 – VoiceMeeter

Steuert VoiceMeeter (Standard, Banana, Potato) über die offizielle Remote-API.

- UUID `de.opendeckn3.voicemeeter`, Ordner `plugins/voicemeeter`, nur Windows (unter Linux baut das Plugin, jede
  Aktion meldet „nur für Windows“, wie AudioSwitcher).
- `VoicemeeterRemote64.dll` zur Laufzeit mit `libloading` laden. Ordner aus der Registry
  (`HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\VB:Voicemeeter {17359A74-1236-5467}`,
  Wert `UninstallString` → Verzeichnis), sonst `C:\Program Files (x86)\VB\Voicemeeter`.
- Funktionen: `VBVMR_Login`, `VBVMR_Logout`, `VBVMR_GetVoicemeeterType`, `VBVMR_IsParametersDirty`,
  `VBVMR_GetParameterFloat`, `VBVMR_SetParameterFloat`, `VBVMR_SetParameters`.
- Alle DLL-Aufrufe in einem eigenen Thread; Hauptschleife spricht per Kanal mit ihm. `IsParametersDirty` alle 100 ms,
  nur dann Werte neu lesen.

## Aktionen

| UUID-Suffix | Name | Controller | Einstellungen | Verhalten |
| --- | --- | --- | --- | --- |
| `gain` | Pegel | Encoder, Keypad | `target` (`Strip[0]` … `Bus[7]`, select), `step` dB (Standard 1) | Drehen: `Gain` −60…+12 dB; Drücken: Mute; Titel `-6,0 dB` |
| `mute` | Stumm | Keypad | `target` | `Mute` umschalten, Zustand 1 = stumm |
| `route` | Routing | Keypad | `strip`, `bus` (`A1`…`A5`, `B1`…`B3`) | `Strip[n].A1` usw. umschalten, Zustand 1 = an |
| `script` | Befehl | Keypad | `script` (textarea) | `VBVMR_SetParameters` mit dem Text, z. B. `Strip[0].Mute=1; Bus[0].Gain=-10;` |

Die Auswahl der Strips/Busse hängt vom Typ ab (Standard 3/2, Banana 5/5, Potato 8/8): Liste per `source` vom Plugin.

## Tests

- Parameternamen bilden (`Strip[2].A1`), Typ → Anzahl Strips/Busse.
- dB-Formatierung und Klemmen.

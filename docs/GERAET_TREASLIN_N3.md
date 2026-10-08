# Gerät: TreasLin N3

| Eigenschaft | Wert |
| --- | --- |
| USB Vendor-ID / Product-ID | `0x5548` / `0x1001` |
| HID Usage Page / Usage | `65440` (`0xFFA0`) / `1` |
| Protokollversion (`mirajazz`) | 3 |
| Display-Tasten | 6 (Raster 3 × 2) |
| Tasten ohne Display | 3 |
| Drehregler (drehen + drücken) | 3 |
| Tastenbild | 64 × 64 px, JPEG, 90° im Uhrzeigersinn gedreht |
| Keep-Alive | alle 15 s |
| Geräte-Id in OpenDeckN3 | `n3-<Seriennummer>` |

Die N3-Familie (Mirabox N3, Ajazz AKP03, Soomfon SE, Mars Gaming MSD-TWO, Redragon SS-551 …)
nutzt dasselbe Protokoll; diese Geräte können später über die Modelltabelle ergänzt werden.

## Layout in OpenDeckN3

Tasten werden zeilenweise (row-major) in einem 3 × 3-Raster nummeriert:

```text
 ┌──────┬──────┬──────┐
 │ K0 ▣ │ K1 ▣ │ K2 ▣ │   ▣ = Taste mit Display (64×64)
 ├──────┼──────┼──────┤
 │ K3 ▣ │ K4 ▣ │ K5 ▣ │
 ├──────┼──────┼──────┤
 │ K6 ○ │ K7 ○ │ K8 ○ │   ○ = Taste ohne Display
 └──────┴──────┴──────┘
   E0 ◎      E1 ◎      E2 ◎   Drehregler: links · mitte (oben) · rechts
```

> ⚠️ Die Zuordnung „links / mitte (oben) / rechts“ der Drehregler und die Reihenfolge der
> Display-Tasten stammt aus `opendeck-akp03` und muss am echten TreasLin N3 verifiziert werden.
> Bei Abweichungen nur `n3-driver/src/n3.rs::process_input` bzw. das Layout anpassen.

## Rohe Eingabe-Codes

Jeder Input-Report beginnt mit `ACK` (`0x41 0x43 0x4B`); Byte 9 ist der Code, Byte 10 der Zustand
(1 = gedrückt, 0 = losgelassen; Protokoll v3 meldet beide Zustände).

| Code | Bedeutung | OpenDeckN3 |
| --- | --- | --- |
| `0x01` … `0x06` | Display-Taste 1–6 | `Keypad` 0–5 |
| `0x25` | Taste ohne Display 1 | `Keypad` 6 |
| `0x30` | Taste ohne Display 2 | `Keypad` 7 |
| `0x31` | Taste ohne Display 3 | `Keypad` 8 |
| `0x90` / `0x91` | Drehregler links: links / rechts | `Encoder` 0, ticks −1 / +1 |
| `0x50` / `0x51` | Drehregler mitte: links / rechts | `Encoder` 1, ticks −1 / +1 |
| `0x60` / `0x61` | Drehregler rechts: links / rechts | `Encoder` 2, ticks −1 / +1 |
| `0x33` | Drehregler links drücken | `Encoder` 0 |
| `0x35` | Drehregler mitte drücken | `Encoder` 1 |
| `0x34` | Drehregler rechts drücken | `Encoder` 2 |

## Ausgabe-Kommandos (über `mirajazz`)

Alle Kommandos beginnen mit `CRT\0\0` (`0x43 0x52 0x54 0x00 0x00`):

| Kommando | Bytes | Zweck |
| --- | --- | --- |
| `DIS` | `44 49 53` | Display initialisieren |
| `LIG` | `4C 49 47 00 00 <pct>` | Helligkeit 0–100 |
| `BAT` | `42 41 54 00 00 <len_hi> <len_lo> <key+1>` + Bilddaten | Tastenbild senden |
| `CLE` | `43 4C 45 00 00 00 <key+1 / 0xFF>` | Tastenbild(er) löschen |
| `STP` | `53 54 50` | Änderungen übernehmen (flush) |
| `HAN` | `48 41 4E` | Schlafmodus |
| `CONNECT` | `43 4F 4E 4E 45 43 54` | Keep-Alive |

## Linux: Zugriffsrechte (udev)

```sh
sudo cp udev/40-opendeckn3.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Danach das Gerät einmal ab- und wieder anstecken.

## Fehlersuche

- `RUST_LOG=n3_driver=trace opendeckn3d` zeigt jeden rohen Input-Code.
- Wird das Gerät nicht gefunden: `lsusb | grep 5548`, udev-Regel prüfen, ggf. Hersteller-Software beenden.
- Gerät ohne Seriennummer wird ignoriert (Log-Warnung) – bitte melden.

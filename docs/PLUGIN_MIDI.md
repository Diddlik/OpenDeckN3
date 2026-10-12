# MIDI-Plugin

Sendet MIDI-Noten, Controller-Werte und Programmwechsel an eine DAW (z. B. Ableton Live oder Reaper) oder an MIDI2LR
für Lightroom Classic. Ein optionaler MIDI-Eingang liefert Rückmeldungen der DAW, damit Tasten leuchten.
Quellcode: [`plugins/midi`](../plugins/midi). Windows und Linux.

## Einrichten

1. **Plugins → Katalog → MIDI → Installieren.**
2. Unter **Plugins → MIDI → Einstellungen** den **MIDI-Ausgang** wählen. Das ist der Port, an dem die DAW den N3 als
   Eingang empfängt.
3. Optional den **MIDI-Eingang** wählen, wenn die DAW Rückmeldungen senden soll (z. B. Schalterzustände).
4. Das Feld **Status** zeigt, ob die Ports offen sind.

Ist ein gewählter Port nicht vorhanden, etwa weil loopMIDI noch nicht läuft, versucht das Plugin alle 5 Sekunden
erneut, ihn zu öffnen. Eine Taste, die ohne offenen Ausgang gedrückt wird, zeigt einen Fehler.

### Windows: loopMIDI

Windows kennt keine virtuellen MIDI-Ports. Dafür nutzt du das kostenlose Programm **loopMIDI** von Tobias Erichsen:

1. loopMIDI installieren und starten.
2. In loopMIDI mit **+** einen Port anlegen, z. B. „OpenDeckN3“. Der Name ist frei wählbar.
3. Im Plugin als **MIDI-Ausgang** diesen Port wählen.
4. In der DAW denselben Port als **MIDI-Eingang** aktivieren, damit sie die Nachrichten empfängt.

loopMIDI muss laufen, bevor OpenDeckN3 startet, oder der Port muss danach angelegt werden. Das Plugin verbindet sich
dann automatisch.

### Linux: virtueller Port

Unter Linux legt das Plugin selbst Ports an. Wähle **Virtueller Port OpenDeckN3** als MIDI-Ausgang; die DAW sieht ihn
danach als MIDI-Eingang „OpenDeckN3“. Für Rückmeldungen wählst du **Virtueller Port OpenDeckN3** auch als
MIDI-Eingang und stellst in der DAW den Ausgang „OpenDeckN3“ ein.

Zum Bauen des Plugins unter Linux werden die ALSA-Header benötigt (`libasound2-dev`, `pkg-config`).

## Aktionen

| Aktion | Taste | Drehregler | Funktion |
| --- | --- | --- | --- |
| **Note** | ✓ | | Beim Drücken Note On, beim Loslassen Note Off. Oder „Umschalten“: jeder Druck wechselt zwischen Note On und Note Off. |
| **Controller (CC)** | ✓ | ✓ | Taste: schaltet den Wert zwischen 0 und 127. Drehregler: siehe unten. |
| **Programmwechsel** | ✓ | | Sendet beim Drücken einen Program Change. |

Kanal ist 1 bis 16, wie in der DAW angezeigt. Notennummer 60 ist C4.

### Drehregler bei Controllern

- **Absoluter Wert:** Jede Raste ändert den gemerkten Wert um den Schritt; der Wert liegt zwischen 0 und 127 und
  wird an die DAW gesendet. Ändert die DAW den Wert selbst, übernimmt das Plugin ihn, sofern der Eingang eingestellt ist.
- **Relativ:** Jede Raste sendet einen Zuwachs im Zweierkomplement um 0: +n als `n`, −n als `128 − n`. Der Zuwachs ist
  auf ±63 begrenzt. Das brauchen DAW-Funktionen, die Drehungen direkt auswerten, z. B. MIDI2LR.

### Rückmeldung

Eingehende Note-On- und Note-Off-Nachrichten sowie Controller mit gleichem Kanal und gleicher Nummer setzen den Zustand
der Taste: Wert größer 0 leuchtet, 0 leuchtet nicht.

## Beispiel: Ableton Live

1. In Ableton unter **Einstellungen → Link, Tempo & MIDI** den Port als Eingang auf **Track** und **Remote** stellen.
2. Oben rechts **MIDI-Map** einschalten (Strg+M).
3. Den Regler oder Knopf in Ableton anklicken, der gesteuert werden soll (z. B. Lautstärke eines Tracks).
4. Die Taste oder den Drehregler am N3 betätigen. Die Zuordnung ist sofort gespeichert.
5. MIDI-Map mit Strg+M wieder ausschalten.

Für Ableton nimmst du am besten die Aktion **Controller (CC)** mit Modus „Absoluter Wert“.

## Beispiel: Reaper

1. **Optionen → Einstellungen → Audio → MIDI-Geräte**: Den Port als Eingang aktivieren. Für die Rückmeldung den Port als
   Ausgang aktivieren, wenn er in der Liste steht.
2. Auf den Parameter (z. B. Fader eines Tracks oder eine FX-Einstellung) rechtsklicken und **Learn** wählen.
3. Die Taste oder den Drehregler am N3 betätigen. Mit **Learn** den Vorgang beenden.

## MIDI2LR für Lightroom Classic

[MIDI2LR](https://github.com/rsanchez/midi2lr) verbindet einen MIDI-Controller mit Lightroom Classic. Das Plugin
stellt dafür die Befehle bereit, die Lightroom braucht.

1. MIDI2LR herunterladen und in Lightroom Classic über **Datei → Zusatzmodul-Manager** hinzufügen. Danach
   MIDI2LR starten.
2. In MIDI2LR den **MIDI-Eingang** auf den Port setzen, den OpenDeckN3 als Ausgang nutzt
   (unter Linux „OpenDeckN3“, unter Windows der loopMIDI-Port).
3. In MIDI2LR eine Funktion wählen (z. B. „Belichtung“ oder „Temperatur“), auf **Learn** klicken und dann die Taste
   oder den Drehregler am N3 betätigen. MIDI2LR übernimmt die Nummer und den Kanal.
4. Für Drehregler die Aktion **Controller (CC)** mit Modus **Relativ** verwenden.

Die Menünamen in Lightroom und MIDI2LR können je nach Version abweichen.

## Hinweise

- Die Werte werden nicht gespeichert. Nach einem Neustart beginnen absolute Controller wieder bei 0, bis die DAW einen
  Wert zurückmeldet.

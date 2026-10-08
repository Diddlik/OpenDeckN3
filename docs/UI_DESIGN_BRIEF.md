# Design-Brief: OpenDeckN3-Oberfläche

> **Zweck dieses Dokuments:** Vorlage für *Claude Design*, um eine moderne, „fancy“ Desktop-Oberfläche
> für OpenDeckN3 zu entwerfen. Es beschreibt Produkt, Nutzer:innen, Bildschirme, Komponenten,
> Zustände und die echten Daten, die die UI anzeigt. Technische Schnittstelle: [UI-API](UI_API.md).

---

## 1. Produkt in einem Satz

Eine Desktop-App, mit der man die Tasten und Drehregler eines **TreasLin N3** Stream-Controllers
per Drag & Drop mit Aktionen belegt, Profile verwaltet und Plugins nutzt – live gespiegelt mit dem echten Gerät.

## 2. Das Gerät (zentrales visuelles Element)

Das N3 ist klein und hat:

- **6 Display-Tasten** (3 × 2, quadratisch, je 64 × 64 px Farbdisplay) – zeigen Icons/Bilder.
- **3 Tasten ohne Display** (Reihe darunter) – nur Funktion, kein Bild.
- **3 Drehregler** mit Druckfunktion (links, mitte/oben, rechts).

```text
        ◎ E1 (mitte/oben)
 ◎ E0                     ◎ E2
 ┌────┐ ┌────┐ ┌────┐
 │ K0 │ │ K1 │ │ K2 │      ← Display-Tasten mit Bild
 └────┘ └────┘ └────┘
 ┌────┐ ┌────┐ ┌────┐
 │ K3 │ │ K4 │ │ K5 │
 └────┘ └────┘ └────┘
  (K6)   (K7)   (K8)       ← Tasten ohne Display
```

*(Exakte Anordnung der Drehregler am Gerät wird noch verifiziert – das Design sollte die
Positionen leicht anpassbar halten.)*

Die UI zeigt eine **stilisierte, realistische Geräteansicht** in der Mitte. Display-Tasten zeigen
die echten Vorschaubilder (`previews`), drücken am Gerät lässt die Taste in der UI aufleuchten
(`input`-Event), Drehen lässt einen Ring um den Drehregler animieren.

## 3. Zielgruppe & Tonalität

- Streamer:innen, Creator, Entwickler:innen, Linux-Power-User.
- Gefühl: **hochwertig, verspielt-technisch, ruhig**. Wie ein Pro-Audio-/Synth-Plugin trifft modernes macOS/Linux-Desktop-Design.
- Sprache der UI: **Deutsch** (Englisch später per i18n), kurze, freundliche Texte.

## 4. Visuelle Richtung (Vorschlag – Claude Design darf frei interpretieren)

- **Dark-first** mit optionalem Light-Theme.
- Hintergrund: tiefes Anthrazit mit subtiler Körnung/Gradient; Gerät als leicht glänzendes
  „Hardware-Objekt“ mit weichem Schatten und Glow unter aktiven Tasten.
- Akzentfarbe: kräftiges Cyan/Türkis oder Violett; Statusfarben: Grün (verbunden/OK), Amber (Warnung), Rot (Fehler).
- Glas-/Blur-Panels für Seitenleisten, abgerundete Ecken (12–16 px), großzügiger Weißraum.
- Typografie: moderne Sans (z. B. Inter, Geist), Monospace für technische Werte (Ids, Ports).
- Micro-Interactions: Tasten „drücken“ sich beim Hover ein, Drop-Ziel leuchtet, sanfte Übergänge beim Profilwechsel (Tasten flippen/blenden).
- Icons: einheitliches Linien-Icon-Set (z. B. Phosphor, Lucide).

## 5. Fensterlayout (Hauptansicht)

Drei Zonen, Mindestgröße ca. 1100 × 700 px, responsiv bis 1920 px:

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ Titelleiste: Logo · Geräteauswahl ▾ · Profil ▾ (+) · Helligkeit ☼──● · ⚙ │
├───────────────┬──────────────────────────────────────┬───────────────────┤
│ AKTIONEN      │                                      │ INSPEKTOR         │
│ 🔍 Suche       │          [ Geräteansicht N3 ]        │ (ausgewählter     │
│ ▸ System      │                                      │  Slot)            │
│   Helligkeit  │   ◎      ◎      ◎                    │ Aktion: Zähler    │
│   Profil …    │  ▣  ▣  ▣                              │ Plugin: …         │
│ ▸ Beispiele   │  ▣  ▣  ▣                              │ Titel  [_____]    │
│   Zähler      │  ○  ○  ○                              │ Bild   [Ändern]   │
│ ▸ Plugin X …  │                                      │ Einstellungen     │
│               │  Live-Status: ● verbunden · FW 1.0   │  { … }            │
│ [+ Plugins]   │                                      │ [Entfernen]       │
└───────────────┴──────────────────────────────────────┴───────────────────┘
```

### 5.1 Titelleiste
- **Geräteauswahl**: Name + Status-Punkt; virtuelle Geräte mit Badge „Virtuell“. Leerer Zustand siehe 7.
- **Profilauswahl**: Dropdown mit allen Profilen, „+ Neues Profil“, Umbenennen/Löschen per Kontextmenü.
  Aktives Profil hervorgehoben.
- **Helligkeit**: Slider 0–100 %, live (`setBrightness`, Event `brightnessChanged`).
- **Einstellungen** (Zahnrad).

### 5.2 Aktionsbibliothek (links)
- Gruppiert nach `category` aus dem Katalog, einklappbar, Suchfeld.
- Jede Aktion: Icon, Name, Tooltip beim Hover, kleine Chips „Taste“/„Drehregler“ (`controllers`).
- Plugin-Status: grauer Punkt wenn `connected = false` (Aktion trotzdem platzierbar).
- Drag & Drop auf einen Slot → `setAction`. Unpassende Slots (Aktion ohne `Encoder` auf Drehregler) sind beim Ziehen ausgegraut.

### 5.3 Geräteansicht (Mitte)
- Realistische Darstellung des N3; Klick auf Slot = auswählen (Inspektor zeigt ihn).
- Display-Tasten: echtes Bild aus `previews[key]`, Titel als Overlay (`titles`/`title`).
- Tasten ohne Display: zeigen das Aktions-Icon klein + Name als Label unter/auf der Taste.
- Drehregler: runde Knöpfe, Aktion als Label, beim Drehen animierter Ring mit Richtungspfeil.
- Leerer Slot: gestrichelter Rahmen mit „+“.
- Rechtsklick-Kontextmenü: Bearbeiten, Duplizieren (später), Entfernen.
- Live-Feedback: `input`-Event → Taste glimmt kurz; `feedback`-Event → kurzes ✓/⚠-Overlay.
- Schalter „Test-Modus“ bei virtuellen Geräten: Klick auf Taste/Drehregler sendet `simulateInput`
  (Mausrad über Drehregler = drehen).

### 5.4 Inspektor (rechts)
- Kopf: Aktions-Icon, Name, Plugin-Name, Version.
- **Darstellung**: Titel-Feld, Bild wählen (Datei → Data-URL) / zurücksetzen (`setActionAppearance`).
- **Einstellungen**: zunächst generischer Editor für `settings` (Formular für Schlüssel/Werte, Fallback JSON-Editor);
  später Platz für die Property-Inspector-Seite des Plugins.
- Eingebaute Aktion „Profil wechseln“: Dropdown mit Profilen (setzt `settings.profile`).
- Gefahrbereich: „Belegung entfernen“ (`clearAction`).

## 6. Weitere Bildschirme

1. **Plugins** – Liste installierter Plugins (Name, Autor, Version, Status, Aktionen-Anzahl);
   Button „Plugin installieren…“ (Datei-Auswahl, kommt in M1); Detailansicht mit Aktionsliste.
2. **Einstellungen** – Allgemein (Sprache, Theme, Autostart), Geräte (Helligkeit, Schlafmodus nach X Min.),
   Erweitert (Ports, Konfigurationsordner öffnen, Log anzeigen), Über (Version, Lizenz, Credits an
   OpenDeck & opendeck-akp03).
3. **Onboarding / Erststart** – 3 Schritte: Willkommen → Gerät anschließen (mit Linux-udev-Hinweis
   und Copy-Button für den Befehl) → erste Aktion per Drag & Drop platzieren.

## 7. Zustände, die gestaltet werden müssen

| Zustand | Darstellung |
| --- | --- |
| Kein Gerät verbunden | Illustration des N3 als Umriss, Text „Schließe deinen TreasLin N3 an“, Link zu udev-Anleitung, Button „Virtuelles Gerät verwenden“ |
| Dienst nicht erreichbar | Vollflächiger Hinweis „Dienst läuft nicht“, Retry-Button, automatischer Reconnect-Spinner |
| Gerät getrennt während Bearbeitung | Geräteansicht ausgegraut + Toast „Gerät getrennt“ |
| Plugin nicht verbunden | Grauer Status-Punkt, Tooltip „Plugin läuft nicht“ |
| Fehler aus API (`ok: false`) | Toast mit Fehlermeldung |
| Leeres Profil | Hinweis-Overlay „Ziehe eine Aktion auf eine Taste“ |
| Profilwechsel | Kurze Übergangsanimation aller Tasten |

## 8. Komponenten-Inventar

`DeviceSelect`, `ProfileSelect`, `BrightnessSlider`, `DeviceView` (`DisplayKey`, `PlainKey`, `Encoder`),
`ActionLibrary` (`CategoryGroup`, `ActionTile`, `SearchField`), `Inspector` (`AppearanceEditor`,
`SettingsEditor`, `DangerZone`), `PluginCard`, `StatusDot`, `Toast`, `EmptyState`, `ContextMenu`,
`ConfirmDialog`, `OnboardingStepper`.

## 9. Beispieldaten für Mockups

```json
{
  "devices": [{
    "info": { "id": "n3-355499441494", "name": "TreasLin N3", "firmware": "V3.0.1",
              "virtualDevice": false,
              "layout": { "rows": 3, "columns": 3, "keys": 9, "displayKeys": 6, "encoders": 3, "keyImageSize": [64, 64] } },
    "brightness": 70,
    "activeProfile": "streaming",
    "profiles": ["default", "streaming", "coding"],
    "profile": {
      "id": "streaming", "name": "streaming",
      "keys": {
        "0": { "plugin": "com.obs.studio", "action": "com.obs.scene", "settings": { "scene": "Kamera" }, "state": 0, "title": "Kamera", "image": null },
        "1": { "plugin": "com.obs.studio", "action": "com.obs.scene", "settings": { "scene": "Screen" }, "state": 0, "title": "Screen", "image": null },
        "2": { "plugin": "com.obs.studio", "action": "com.obs.record", "settings": {}, "state": 1, "title": "REC", "image": null },
        "3": { "plugin": "de.opendeckn3.counter", "action": "de.opendeckn3.counter.count", "settings": { "count": 12 }, "state": 0, "title": null, "image": null },
        "6": { "plugin": "opendeckn3.builtin", "action": "opendeckn3.builtin.profile", "settings": { "profile": "coding" }, "state": 0, "title": null, "image": null }
      },
      "encoders": {
        "0": { "plugin": "com.audio.mixer", "action": "com.audio.volume", "settings": { "target": "Master" }, "state": 0, "title": null, "image": null },
        "1": { "plugin": "opendeckn3.builtin", "action": "opendeckn3.builtin.brightness", "settings": {}, "state": 0, "title": null, "image": null }
      }
    },
    "previews": {}, "titles": { "3": "12" }
  }],
  "catalog": [
    { "uuid": "opendeckn3.builtin", "name": "OpenDeckN3", "category": "System", "connected": true,
      "actions": [
        { "uuid": "opendeckn3.builtin.brightness", "name": "Helligkeit", "controllers": ["Keypad", "Encoder"] },
        { "uuid": "opendeckn3.builtin.profile", "name": "Profil wechseln", "controllers": ["Keypad"] } ] },
    { "uuid": "de.opendeckn3.counter", "name": "Zähler (Beispiel)", "category": "Beispiele", "connected": true,
      "actions": [ { "uuid": "de.opendeckn3.counter.count", "name": "Zähler", "controllers": ["Keypad", "Encoder"] } ] }
  ]
}
```

## 10. Technische Leitplanken für die Umsetzung

- Ziel-Stack (Vorschlag): **Tauri 2 + Svelte 5 oder React + TypeScript**, Tailwind/CSS-Variablen für Design-Tokens.
- Die UI **hält keinen eigenen Zustand** außer UI-Zustand (Auswahl, offene Panels). Quelle der Wahrheit ist
  der Dienst: initial `getState`, danach Events einarbeiten; bei `slotChanged`/`lagged` neu laden.
- Alle Bilder kommen als Data-URLs; keine Dateipfade vom Dienst.
- Muss mit dem **virtuellen Gerät** (`opendeckn3d --virtual --no-hardware`) vollständig bedienbar sein.
- Barrierefreiheit: Tastaturnavigation über alle Slots (Pfeiltasten), Fokus-Ringe, Kontrast AA.

## 11. Was wir von Claude Design erwarten

1. Moodboard/Design-Tokens (Farben, Typo, Radien, Schatten, Animationen) für Dark & Light.
2. High-Fidelity-Mockups: Hauptansicht (belegt), Hauptansicht (leer), Drag-&-Drop-Zustand, Inspektor
   für eingebaute Aktion, Plugins-Seite, Einstellungen, Onboarding (3 Schritte), „Kein Gerät“-Zustand.
3. Klickbarer Prototyp der Hauptansicht.
4. Komponentenbibliothek gemäß Abschnitt 8.

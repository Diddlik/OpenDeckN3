# Weboberfläche

`index.html` ist die Oberfläche von OpenDeckN3 – eine einzelne Datei ohne Build-Schritt und ohne
Abhängigkeiten (Schriften kommen von Google Fonts, ohne Internet greift die Systemschrift).

- **Design:** Entwurf aus Claude Design, Original unter [`design/prototyp-claude-design.html`](design/prototyp-claude-design.html)
  (mit Beispieldaten, zum Durchklicken im Browser). Grundlage war [`docs/UI_DESIGN_BRIEF.md`](../docs/UI_DESIGN_BRIEF.md).
- **Daten:** ausschließlich über die UI-API ([`docs/UI_API.md`](../docs/UI_API.md)). Die Oberfläche hält nur
  UI-Zustand (Auswahl, Theme …); Quelle der Wahrheit ist der Dienst (`getState` + Live-Events).
- **Auslieferung:** `opendeckn3d` bettet die Datei beim Kompilieren ein und liefert sie unter
  `http://127.0.0.1:57132/` aus. Dabei wird `{{API_PORT}}` durch den API-Port ersetzt.

## Starten

```sh
cargo run -p n3-daemon -- --virtual --no-hardware --plugins-dir plugins/examples
# Browser: http://127.0.0.1:57132/
```

Nach Änderungen an `index.html` den Dienst neu bauen (`cargo build`), da die Datei eingebettet wird.

## Funktionsumfang

| Bereich | Stand |
| --- | --- |
| Editor: Geräteansicht mit Live-Vorschau, Drag & Drop, Klick-Zuweisung, Kontextmenü, Tastaturnavigation | ✅ |
| Inspektor: Titel, eigenes Bild (wird auf 144 px PNG verkleinert), Einstellungen als Felder oder JSON, Entfernen | ✅ |
| Profile: wechseln, anlegen, löschen; Helligkeit | ✅ |
| Test-Modus beim virtuellen Gerät: Klick drückt, Mausrad dreht (`simulateInput`) | ✅ |
| Zustände: Dienst offline (Auto-Reconnect), kein Gerät, Gerät getrennt, Onboarding | ✅ |
| Plugins-Seite (Katalog, Status) und Einstellungen (Theme, Gehäusefarbe, API-Port) | ✅ |
| Plugin-Installation, Autostart, Schlafmodus, Property Inspector | folgt (M1/M2) |

# Hinweise für Claude / Mitwirkende

- Projektziel und Begriffe: `docs/VORHABEN.md`; Aufbau: `docs/ARCHITEKTUR.md`; Technik-Übersicht: `docs/ENTWICKLUNG.md`.
- `README.md` ist die Produktseite (Marketing, wenig Technik). Technisches gehört nach `docs/`. Screenshots in
  `docs/images/` mit `tools/screenshots/` neu erzeugen, wenn sich die Oberfläche sichtbar ändert.
- Rust-Workspace (Edition 2024). Vor jedem Commit:
  `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`
- End-to-End ohne Hardware:
  ```sh
  cargo build
  ./target/debug/opendeckn3d --virtual --no-hardware --plugins-dir plugins/examples --config-dir /tmp/n3cfg &
  node tools/smoke-test.mjs
  ```
- Der gesamte Fachzustand liegt in `crates/n3-daemon/src/app.rs` (`App`) und wird nur aus dem
  Haupt-Loop verändert – keine Locks einführen, sondern Kanäle nutzen.
- Neue UI-Kommandos/Events immer auch in `docs/UI_API.md` dokumentieren, neue Plugin-Events in `docs/PLUGIN_API.md`.
- Weboberfläche: `ui/index.html` (eine Datei, kein Build, wird per `include_str!` eingebettet → nach Änderungen
  `cargo build`). Design-Referenz: `ui/design/prototyp-claude-design.html`. UI im Browser: `http://127.0.0.1:57132/`.
- Desktop-App: `crates/n3-desktop` (Tauri 2), nicht in den `default-members` → `cargo build -p n3-desktop`
  (Linux braucht `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev`). Lädt `ui/` als Frontend, startet den Dienst über
  `n3_daemon::run`. Fenster-Option `dragDropEnabled` muss `false` bleiben – sonst fängt WebView2 unter Windows
  Drag & Drop ab und Aktionen lassen sich nicht auf Tasten ziehen. Icons: `python3 tools/make-icon.py`; Icons der eingebauten Aktionen (Gerät):
  `NODE_PATH=$(npm root -g) node tools/make-builtin-icons.cjs` (aus `ICONS`/`KCOL` in `ui/index.html`). Windows-Installer baut `.github/workflows/windows.yml`.
- Doku auf Deutsch, Code/Kommentare auf Englisch.

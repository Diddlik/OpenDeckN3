# Hinweise für Claude / Mitwirkende

- Projektziel und Begriffe: `docs/VORHABEN.md`; Aufbau: `docs/ARCHITEKTUR.md`.
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
- Doku auf Deutsch, Code/Kommentare auf Englisch.

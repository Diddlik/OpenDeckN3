# UI (geplant)

Hier entsteht die grafische Oberfläche von OpenDeckN3 (Meilenstein M1).

- **Design-Grundlage:** [`docs/UI_DESIGN_BRIEF.md`](../docs/UI_DESIGN_BRIEF.md) – Vorlage für Claude Design.
- **Schnittstelle:** [`docs/UI_API.md`](../docs/UI_API.md) – WebSocket `ws://127.0.0.1:57131`.
- **Entwickeln ohne Gerät:**

  ```sh
  cargo run -p n3-daemon -- --virtual --no-hardware --plugins-dir plugins/examples
  ```

Vorgeschlagener Stack: Tauri 2 + Svelte 5 (oder React) + TypeScript. Die UI hält keinen eigenen
Fachzustand, sondern spiegelt den Dienst (`getState` + Events).

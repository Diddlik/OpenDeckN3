# Shared Agent Instructions

This file is the canonical instruction source for Codex, Claude Code, and GitHub Copilot CLI.

## Maintenance

- Keep this file synchronized with the repository's verified behavior.
- Update this file in the same change when build commands, validation steps, architecture constraints, workflows, or conventions change.
- Remove obsolete instructions instead of appending corrections.
- Record only durable facts that are not obvious from the codebase.
- Replace an `OPEN` entry as soon as the decision is made or the fact becomes verifiable.
- Tool-specific instruction files contain only a pointer to this file and genuine tool-specific exceptions.

## First-use onboarding

If any `[TO FILL]` entry remains, complete this onboarding before implementing the user's first task:

1. Inspect the repository and determine every project fact that can be verified from existing files and commands.
2. Do not ask the user for information that can be discovered reliably from the repository.
3. Briefly present the discovered facts, then ask guided questions only for the remaining decisions or unknowns.
4. Ask one focused question or one closely related group of no more than three questions at a time. Explain why each answer matters and offer a recommended option when useful while allowing a free-form answer.
5. Cover the remaining topics in the order of the project facts list: purpose and scope, languages, runtimes and platforms, GUI, build and validation commands, environment requirements, then entry points, architecture, and generated-file constraints.
6. After the user answers, replace the applicable placeholders with concise verified facts, remove entries that do not apply, and record unresolved decisions explicitly as `OPEN` rather than inventing an answer.
7. Summarize what was written to this file, then continue with the user's original task.

If the user cannot be asked (non-interactive run, CI, issue-driven agent, subagent), fill every verifiable fact, mark the rest `OPEN`, and proceed with the task.

## Communication

- Respond to the user in the language the user writes in, including onboarding questions.
- Project artifacts stay in English as defined below.

## Implementation

- Implement only what the current requirement needs.
- Prefer editing existing code over adding files, layers, helpers, or abstractions.
- Use the standard library and existing dependencies before writing custom implementations.
- Do not add speculative extension points, configuration, parameters, or abstractions.
- Add a dependency only when it provides a concrete benefit and does not duplicate existing functionality.
- Preserve validation, security, accessibility, error handling, and data-integrity safeguards.

## Language and comments

- Use English for source code, identifiers, comments, tests, documentation, logs, and commit messages, except where the project facts define a documentation-language exception.
- Comments explain why a decision, constraint, workaround, or non-obvious trade-off exists.
- Do not comment what readable code already expresses.
- Prefer clear naming and small functions over explanatory comments.

## Git

- Commit coherent, verified units of work such as a finished feature, fix, or refactoring step, not every individual edit.
- Group related changes into one commit; keep unrelated changes in separate commits.
- Keep commit messages limited to change- and process-related content.
- Never add AI attribution or co-author trailers such as `Co-Authored-By: Copilot`, Claude, Codex, or similar, in commits or pull requests.

## GUI applications

Apply this section when the project has or adds a graphical user interface.

Required features:

- Versioning: one single source for the version number (Semantic Versioning), shown in the UI and embedded in build artifacts.
- About dialog: application name, version, copyright, license, and credits for every third-party library with its license.
- Auto-update: check for updates over HTTPS, verify the update's signature or checksum before installing, let the user postpone or disable automatic checks, and never lose unsaved data during an update.

Design:

- Plan the layout, navigation, and main user flows before implementing screens.
- Follow the platform's current design system: Fluent 2 on Windows, Apple Human Interface Guidelines on macOS and iOS, Material 3 on Android and where no platform system applies.
- Use native or toolkit-standard controls instead of custom widgets.
- Use a consistent spacing grid (multiples of 4 or 8 px), a limited type scale, and the system font unless the brand requires otherwise.
- Support light and dark mode and follow the system setting by default.
- Keep a clear visual hierarchy: one primary action per view, secondary actions visually subordinate, generous whitespace instead of borders and boxes.
- Design empty, loading, and error states; errors say what happened and what the user can do.
- Keep the UI responsive: run long operations in the background with progress indication and cancellation.
- Prefer undo over confirmation dialogs for reversible actions.
- Adapt to window sizes and display scaling (high DPI) without clipping or overlapping content.
- Accessibility: meet WCAG 2.2 AA contrast, provide full keyboard navigation with visible focus, label controls for screen readers, and respect reduced-motion settings.
- Keep animations short and purposeful.
- Keep user-facing strings out of code logic so they can be localized.
- Remember window size, position, and user preferences between sessions.

## Working method

- Inspect the relevant implementation and existing conventions before editing.
- Search for an existing implementation before creating a new one.
- Make the smallest coherent change that fully satisfies the request.
- Preserve unrelated user changes.
- Do not perform unrelated refactoring during a focused change.
- Ask before destructive, irreversible, security-sensitive, or materially out-of-scope actions.

## Verification

- Run the narrowest relevant test, build, lint, format, or executable check after the last change.
- Add or update tests for non-trivial behavior changes and bug fixes.
- Do not claim success without current verification evidence.
- Report what was verified and what could not be verified.
- Distinguish product failures from environment, permission, network, and tooling failures.

## Security

- Never commit, print, store, or document secrets, tokens, credentials, or private keys.
- Validate data at user, file, environment, process, and network boundaries.
- Preserve authentication, authorization, escaping, permission checks, and safe defaults.
- Do not weaken security controls to make tests or local execution pass.

## Project facts

- Purpose and scope: open-source replacement for the TreasLin N3 stream-deck software (keys, dials, profiles, plugins). Goals, non-goals, terms: `docs/VORHABEN.md`; architecture: `docs/ARCHITEKTUR.md`; technical overview: `docs/ENTWICKLUNG.md`.
- Primary languages and runtimes: Rust workspace (edition 2024, MSRV 1.88); web UI in plain HTML/JS; example plugin and tools in Node.js >= 22.
- Platform-specific constraints: Windows and Linux. Linux device access needs `udev/` rules. Desktop app on Linux needs `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev`; on Windows it needs WebView2.
- GUI toolkit, update mechanism, and distribution: Tauri 2 desktop app (`crates/n3-desktop`) loading `ui/` as frontend. Auto-update (`crates/n3-desktop/src/updater.rs`) checks GitHub releases and verifies the installer against the release's `SHA256SUMS.txt`. Windows installer and portable ZIP are built by `.github/workflows/windows.yml`. The app version comes from `[workspace.package] version` in `Cargo.toml`; release tags (`vX.Y.Z`) must match it because the workflow stamps the tag version into the installer. The UI shows it via `getState.version` (desktop: `desktop_paths.version`).
- Build command: `cargo build`; desktop app: `cargo build -p n3-desktop` (not in `default-members`), installer: `cd crates/n3-desktop && npx @tauri-apps/cli@2 build`.
- Test command: `cargo test`; end-to-end without hardware:
  ```sh
  cargo build
  ./target/debug/opendeckn3d --virtual --no-hardware --plugins-dir plugins/examples --config-dir /tmp/n3cfg &
  node tools/smoke-test.mjs
  ```
- Lint and format command (run before every commit, CI enforces it): `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`; desktop: `cargo clippy -p n3-desktop --all-targets -- -D warnings`. CI uses the latest stable toolchain, so new Clippy lints can fail there first; keep the local toolchain current (`rustup update`).
- Local run command: `cargo run -p n3-daemon -- --virtual --no-hardware` or the desktop app; UI in the browser at `http://127.0.0.1:57132/`.
- Required environment: stable Rust with rustfmt and clippy; Node.js >= 22 for the smoke test and icon tools.
- Important entry points: daemon CLI `opendeckn3d` (`crates/n3-daemon/src/main.rs`, library entry `n3_daemon::run`), desktop `crates/n3-desktop/src/main.rs`, UI `ui/index.html`, plugins under `plugins/`.
- Architecture constraints:
  - All domain state lives in `App` (`crates/n3-daemon/src/app.rs`) and is mutated only from the main loop. Use channels, never add locks.
  - Document new UI commands/events in `docs/UI_API.md` and new plugin events in `docs/PLUGIN_API.md`.
  - `ui/index.html` is a single file without a build step, embedded via `include_str!`; rebuild with `cargo build` after changes. Design reference: `ui/design/prototyp-claude-design.html`.
  - The Tauri window option `dragDropEnabled` must stay `false`; otherwise WebView2 on Windows intercepts drag & drop and actions cannot be dropped onto keys.
  - `README.md` is the product page (marketing, little technical detail); technical content goes into `docs/`.
- Documentation language (exception to the English rule): the product page exists twice, `README.md` (English, GitHub landing page) and `README.de.md` (German); keep both in sync and linked to each other. `docs/` is German.
- Generated files: app icons (`python3 tools/make-icon.py`); built-in action icons (`NODE_PATH=$(npm root -g) node tools/make-builtin-icons.cjs`, from `ICONS`/`KCOL` in `ui/index.html`); plugin key images (`tools/make-discord-icons.cjs`, `tools/make-plugin-icons.cjs`); screenshots in `docs/images/` (regenerate with `tools/screenshots/` when the UI visibly changes).
- Files or directories not to edit manually: the generated icons and screenshots above; regenerate them with their tools.

## Definition of done

A change is complete when:

- The requested behavior is implemented.
- Relevant verification passes after the final edit.
- Appropriate error paths and edge cases are handled.
- Documentation and this file reflect changed behavior or workflows.
- No unrelated files, abstractions, or dependencies were introduced.
- Remaining limitations and unverified points are stated clearly.

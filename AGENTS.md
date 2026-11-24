# Orivo — notes for coding agents

Orivo is a local-first game library: a Tauri v2 app with a vanilla TypeScript
frontend (no framework) and a Rust host. It ships on macOS, Windows, Linux,
Android and iOS, plus a sandboxed Mac App Store edition (`--features app-store`).

Read this first, then the doc for the area you touch. `CONTRIBUTING.md` holds
the house style and the worktree setup; this page is the short version.

## Commands

```sh
pnpm install
pnpm dev               # frontend alone, in a browser, on fixture data (src/mock-library.ts)
pnpm tauri dev         # the desktop app ("Orivo Dev", its own data folder)
pnpm check             # what CI runs: typecheck + Vitest, then vite build + cargo test --workspace
pnpm check:web         # typecheck + Vitest only
pnpm check:rust        # vite build + cargo test --workspace only
pnpm test:e2e          # Playwright against `pnpm dev`; separate, takes minutes
pnpm store:build       # regenerate the Store catalogue (src/generated/)
```

Run `pnpm check` before every push. `cargo` needs `dist/` to exist
(`tauri::generate_context!` embeds it), which `check:rust` builds first; for a
bare `cargo` command, run `pnpm exec vite build` once.

Device runs: `scripts/run-android-device.sh`, `scripts/run-android-emulator.sh`,
`scripts/run-ios-simulator.sh`, `scripts/run-windows-wine.sh`,
`scripts/run-linux-docker.sh`.

## Map

| Path | What lives there |
| --- | --- |
| `src/main.ts` | Boot: form factor, motion, language, splash, then `mountApp` |
| `src/app.ts` | The app shell and most screens (large; being split into per-route modules) |
| `src/router.ts`, `src/page-lifecycle.ts`, `src/spatial-nav.ts` | Routes, page mount/unmount, controller focus |
| `src/*-page.ts` / `src/*-model.ts` | A screen's DOM and IPC / its pure logic (test the model) |
| `src/contracts.ts` | Shapes shared with the host |
| `src/i18n.ts`, `src/locales/` | Every visible string, via `t()` (see `docs/i18n.md`) |
| `src/generated/` | Script output. Never edit by hand |
| `src-tauri/src/lib.rs` | `run()`, app setup, `tauri::generate_handler![…]` and many commands (large; being split) |
| `src-tauri/src/catalog.rs` | The library on disk (`catalog.json`, `game-state.json`) |
| `src-tauri/src/store.rs` | Store pages and the embedded catalogue |
| `src-tauri/src/source_*.rs`, `sources.rs` | Store accounts and library sync |
| `src-tauri/src/*runner*.rs`, `console_*.rs`, `gamestream.rs` | Launching: Wine, Winlator, emulators, streaming |
| `src-tauri/src/plugin_*.rs`, `wit/`, `sdk/` | The Wasm plugin host, its contract and SDK |
| `src-tauri/src/platform.rs` | What each platform and edition may do |
| `quiky-host/` | Separate crate for Quiky installs |
| `e2e/` | Playwright specs; goldens in `e2e/__screenshots__` are rendered on macOS |

Specifications: `docs/DESIGN.md` (UI), `docs/ARCHITECTURE.md` (boundaries;
its §7.1 SQLite is a target, not the present), `docs/selector-contract.md`
(focus), `docs/RELEASING.md`, and one doc per runner or platform in `docs/`.

## Invariants

- **Credentials never reach the WebView.** Tokens live in the system keychain
  and are read by Rust. No IPC command returns a secret.
- **The frontend calls the host by name.** A new command goes in
  `generate_handler![…]` in the same change as its first `invoke("…")`.
  `src/ipc-contract.test.ts` fails otherwise; a command registered but not
  called from `src/` must be listed there.
- **Generated files are not edited.** Change the script and rerun it. Files in
  `src/generated/` open with `GENERATED FILE — DO NOT EDIT`; the Store
  catalogue also has a JSON twin in `src-tauri/resources/` that must match.
- **Every visible string goes through `t()`**, English first; a missing French
  key falls back to English.
- **UI follows `docs/DESIGN.md`.** Panels stay compact: short labels, states as
  pills, buttons as verbs, no paragraphs; how-tos go in `docs/`, not on screen.
  Reuse an existing component before adding one.
- **Platform differences go through `platform.rs` / `platform-features.ts`**,
  and the `app-store` feature removes downloaded plugins, runners and Quiky.
- **Do not hand-edit** `src-tauri/patches/` (vendored tao/wry/tauri-runtime-wry
  fixes, an iOS launch crash among them) or `src-tauri/gen/android/` beyond
  what `docs/RELEASING.md` describes.

## Tests

- Vitest tests sit next to their file (`foo.ts` + `foo.test.ts`) and run in
  jsdom. Test the `*-model.ts` logic; mock IPC at the `invoke` boundary.
- Rust tests are `#[cfg(test)] mod tests` inside each module; quiky-host and
  the SDK also have `tests/` folders. Perf benches are `#[ignore]`d.
- A behaviour change or a bug fix comes with a test that fails without it.
- A test you write must fail when the code it covers is broken; check it.
- Never disable or skip a failing test to get green. Fix it, or say so.

## Changes and pull requests

- One concern per pull request. A refactor that moves code changes no
  behaviour; a behaviour change is its own PR.
- Match the file's comment voice: comments say why, not what.
- No new dependency without saying in the PR what it buys and costs, and its
  licence (MIT, Apache-2.0, BSD, ISC, Zlib only; no copyleft).
- Work in your own git worktree with its own `target/`; never share
  `CARGO_TARGET_DIR` between worktrees.

## Pitfalls

- `pnpm dev` and Playwright run on mock IPC: green e2e says nothing about the
  native paths (launching, Moonlight/Artemis hand-off, Winlator, Android, iOS).
  Say what you could not check on a device.
- Visual goldens are macOS renders; do not update them from Linux.
- A desktop dev build uses `io.orivo.desktop.dev`, not the installed app's
  library. Copy `catalog.json` across to test on real data.
- `innerHTML` with catalogue or store text: escape it. Data from stores and
  plugins is untrusted.

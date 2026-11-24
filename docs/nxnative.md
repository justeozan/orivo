# Native Switch (Switch Natif) — proof of concept

[nxnative](https://github.com/seedds/nxnative) (MIT) packs a Switch game the
player owns into a Mac app that runs the game's own code natively on Apple
silicon, with its graphics rebuilt on Metal. It is not an emulator, and Orivo
ships none of it.

This proof of concept makes Orivo a front end for it, the way NX2Apple and NX
Play are, by speaking the contract nxnative documents for front ends
(`docs/frontends.md` in that repository) rather than any front end's internals.
It is a **built-in** plugin (Settings › Plugins › Native Switch, « Switch Natif » in French), like
Wine, not a WebAssembly component: everything it does is starting processes
and writing another app's preferences, which the `runner@1` sandbox
deliberately cannot do.

## What it does

| Piece | How | Where |
| --- | --- | --- |
| Recognises a built app | `DMTitleID` in the app's `Info.plist` → the card reads `Nintendo Switch · <title ID>`, as a Ryujinx card does | `nxnative::inspect_bundle`, called from `Game::from_executable` |
| Checks the Mac | Apple silicon, macOS ≥ 26.6, Xcode 27.2 selected, the Metal toolchain, Homebrew, `xcodegen`/`zstd`/`glslang`, an arm64 Python ≥ 3.12, its five modules, a checkout. Only what is missing is drawn, each with the button that fixes it, run inside Orivo with its log | `nxnative::doctor`, `nxnative_run_fix`, `nxnative_install_all` |
| Gets nxnative | `git clone --depth 1` into Orivo's data folder (or `git pull --ff-only` when it is there), or the player's own checkout through a folder picker | `nxnative_clone`, `nxnative_choose_checkout` |
| Builds a game | `build_ipa_from_nsp.py <folder> [--platform mac-app] [-k prod.keys] --progress json`, one process, no shell; each JSON line becomes a typed event; the exit code is the documented one; SIGTERM is Cancel | `nxnative::build_command`, `parse_event`, `outcome`; `nxnative_build`, `nxnative_cancel_build` |
| Adds the result | the `game` event's `out` is kept host-side; a finished Mac build is imported like a picked `.app` | `add_built_app` |
| Per-game settings | `Contents/Resources/dm/launcher.json` (schema 1) drawn as selects, toggles and sliders; a change is written to the app's saved choices with `defaults write <bundle id> launcher.<ENV>`, and choosing the tested default deletes the key, as the contract asks | `nxnative_games`, `nxnative_set_choice` |

## Folder, Update, Remove

| Piece | How | Where |
| --- | --- | --- |
| nxnative's folder | Missing at the first open: Orivo clones it into its data folder by itself, once (the row's Download is the retry). Always on the nxnative row: a pill with the folder's name, Update, and a folder icon that changes it to the player's own checkout | `shouldAutoClone`, `nxnative_choose_checkout` |
| Add a game | The library's "Add a folder": the picked folder is added to the games folders (no scan of its own; the background scan finds what is built or updated in it) and built for this Mac | `remember_game_folder`, `nxnative_build` |
| Update | `git pull --ff-only` on nxnative (a clone when there is none), streamed under the row, then a scan of every games folder, reported like the library's own. The same folders are already watched every few minutes; the button is "now" | `nxnative_update` |
| Remove | Deletes what Orivo made (its clone, its Python environment), forgets the player's own checkout and `prod.keys` (never deletes them), keeps built games in the library, and moves the card to the catalogue, where Add brings it back. Refused while a build or an install runs | `nxnative_remove`, `nxnative_restore`, `nxnative_installed` |

## Playing, as on a console

Play (on the card, or beside the game in the panel) goes straight to the game,
on the desktop Orivo is on, and Orivo steps aside only once the game is on
screen. It works the same for an app nxnative's scripts, NX2Apple or NX Play
packed: nothing is rebuilt.

| Piece | How | Where |
| --- | --- | --- |
| No menu in between | `DM_LAUNCHER=0`: the app's own settings window is skipped (the contract's switch); the panel draws the settings and writes the same saved choices | `nxnative::console_env` |
| Same desktop | `DM_FULLSCREEN=1`, and Orivo's small library loaded into the game (`DYLD_INSERT_LIBRARIES`): the game window covers its screen without a title bar, menu bar and Dock hidden while it is active, instead of the Space macOS fullscreen makes. ⌃⌘F still gives the window back. Packed apps have no hardened runtime (the game's code is mapped at run time), so the library loads; it only changes the window | `src-tauri/src/nxnative_fill.m` (built by `build.rs`, arm64, written to the data folder by content hash) |
| Orivo leaves when the game is up | the launch answers `handoff: "window"`; the host reads the window list (owner ids only, no permission) until the launcher or its child has a window, hands it the keyboard, and emits `game-window`; the splash steps aside then, or goes back to Orivo if the game ended first | `nxnative_commands::watch_console_start`, `launch-splash.ts` |
| Quit prompt in French | `DM_HOME_QUIT_TEXT`, when Orivo is read in French | `nxnative::console_env` |

Display mode is therefore Orivo's, and the panel no longer draws it.

## What it never does

- **Open `prod.keys`.** The player picks it once; its path is passed to the
  build as `-k`, and remembered in `nxnative.json` so the next build does not
  ask. Orivo never reads the file.
- **Send a path to the page.** The WebView sees check states, folder names,
  titles and build events. The `game` event's `out` is `#[serde(skip)]`, and a
  test holds it to that.
- **Run a shell.** Every command is a program and an argument list. Bundle ids
  handed to `defaults` are held to reverse-DNS characters and may not begin
  with `-`; a saved value has to be one the app's own list offers.
- **Install the toolchain behind the player's back.** The checklist draws only
  what is missing, one row with one button, and "Install all" walks them in
  order (`FIX_ORDER`). Each fix runs inside Orivo (`nxnative_run_fix`,
  `nxnative_install_all`): the page names the check, the host runs the command
  it holds for it, unattended (`yes` on its input, `NONINTERACTIVE=1`,
  `PIP_NO_INPUT=1`), and streams its output as `nxnative-fix` lines under the
  row, the full log behind a toggle. Selecting Xcode goes through macOS's
  administrator password sheet. An item counts as fixed only when the
  checklist says so afterwards. Xcode and Homebrew stay download pages
  (Homebrew's installer refuses to run as root). Fix logs are diagnostic
  output and may name paths; the checklist itself still does not.

## Editions

The panel exists only in builds that can start processes on macOS: development
builds and the Developer ID download. The App Store edition runs in the App
Sandbox, where Python, Xcode and Homebrew cannot be started and another app's
preferences domain cannot be written, so `nxnative_status` answers
`supported: false` there and the card is not shown. A built app dropped in a
games folder is still recognised as a Switch game in every edition.

## What was verified, and what was not

- Rust: 16 tests in `nxnative.rs`. They cover every documented event, the
  exit codes, the argument list, the checklist on a ready and a fresh Mac
  (through a fake probe), the schema-1 settings list, saved-choice validation,
  a fake nxnative bundle imported as a Switch card, and a real Python process
  standing in for the packing script, whose stream is parsed to its exit code.
- TypeScript: `src/nxnative-panel.test.ts` covers the build view reducer, the
  checklist and the settings visibility rules.
- **Run live (2026-11-21)**: the panel was opened in a running Orivo; an
  NX2Apple-packed Ocarina of Time started with `DM_LAUNCHER=0` (its log: "no
  settings window") and, with the library loaded, its window covered the
  screen (0,0 1512×982) with the other apps' windows still on the same
  desktop. A stand-in window checked the library's toggles (startup request
  keeps it covering, ⌃⌘F gives the window back and covers again).
- **Not run live.** No real game has been built through Orivo yet. The checklist's probes were run by
  hand on one Mac (macOS 27.2.1, Xcode 27.2), where they found `glslang` and
  the Python modules missing, as a fresh setup would.

## Seams

- **Python modules.** Homebrew's Python refuses `pip install` (PEP 668), so
  the button makes Orivo's own environment (`nxnative-python` in Orivo's data
  folder, from the system's Python) and installs the modules there; the
  checklist and the build then use that Python. Run for real on Python 3.14
  (all five modules found afterwards).
- **iPhone.** An IPA build is offered and left beside the game; installing it
  is the player's sideloading tool's job.
- **Cloud saves and cheats.** The contract documents both (`cloudSaves.folder`,
  the cheats folder); neither is drawn yet.
- **Following main.** nxnative has no releases. The clone is shallow and
  updated with `--ff-only`; pinning a tested commit, and reading
  `Front-ends:` trailers between two commits, is what a real integration
  would add.

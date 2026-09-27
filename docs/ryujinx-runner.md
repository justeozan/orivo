# The Ryujinx runner

Ryujinx emulates the Nintendo Switch. Orivo's runner for it is the first
**official plugin shipped as a WebAssembly component** rather than as a native
Rust adapter: it lives in [`plugins/ryujinx/`](../plugins/ryujinx), it is built
by its own `build.sh`, and the host loads it through exactly the same door a
third party's package goes through — `plugin_installer` → `plugin_registry` →
`plugin_runtime` → `runner_host`. Wine‑Staging and Winlator honour the same
`runner@1` contract in Rust inside this binary
([`docs/winlator-runner.md`](winlator-runner.md)); this one is guest code under
the sandbox, which is the point. It answers step 2.1 of
[`docs/plugin-system-plan.md`](plugin-system-plan.md).

**Orivo never ships Ryujinx, a firmware dump, a key file or a game.** The user
installs Ryujinx themselves and points Orivo at it.

## What the plugin does, and what it can never do

It has one job: look at the names of the files in the one folder the user
allowed, and say which of them are Switch games.

| It does | It never does |
| --- | --- |
| calls `host-files.list-directory` on the single grant `games` | calls `host-files.read-file` — **at all** |
| returns a title, a title id and an opaque reference per game | returns a path, a command, or an argument |
| returns a typed `LaunchIntent` in the one declared mode | chooses the executable, the arguments or the working directory |
| declares `runner_prepare` and `files_read` | declares `network_fetch`, `secrets` or any network domain |

The second row is the one that matters. A Switch library folder usually also
holds `prod.keys` and `title.keys` — the files that decrypt every dump the user
owns — and often their saves. This plugin cannot read any of them, because it
never reads a file at all: a name is the only input it takes. That is not a
promise in a document, it is the host's own accounting.
`src-tauri/src/ryujinx_plugin.rs`'s
`nothing_beside_the_games_is_offered_and_no_file_is_ever_read` asserts
`InvocationCost::bytes_read == 0` after a full page of discovery.

There is no WASI in the component either — no clock, no random source, no
socket, no preopened directory — because the host refuses to instantiate a
component that imports one (`wit/README.md`).

## What Ryujinx accepts, and where that list was read

Everything below comes from Ryujinx's own source and its own macOS bundle, not
from a guide. The upstream repository was taken down in October 2024; the files
were read from a mirror of `master`, and the paths are the upstream ones.

`src/Ryujinx.UI.Common/App/ApplicationLibrary.cs` filters the folders it scans
with, verbatim:

```csharp
(Path.GetExtension(file).ToLower() is ".nsp" && …ShownFileTypes.NSP.Value) ||
(Path.GetExtension(file).ToLower() is ".pfs0" && …ShownFileTypes.PFS0.Value) ||
(Path.GetExtension(file).ToLower() is ".xci" && …ShownFileTypes.XCI.Value) ||
(Path.GetExtension(file).ToLower() is ".nca" && …ShownFileTypes.NCA.Value) ||
(Path.GetExtension(file).ToLower() is ".nro" && …ShownFileTypes.NRO.Value) ||
(Path.GetExtension(file).ToLower() is ".nso" && …ShownFileTypes.NSO.Value)
```

and `src/Ryujinx.UI.Common/Configuration/FileTypes.cs` is that list as an enum:
`NSP, PFS0, XCI, NCA, NRO, NSO`. `TryGetApplicationsFromFile` in the same file
switches on the same six. No `.kip`, despite what several third‑party guides
say.

`distribution/macos/Info.plist` declares a narrower set in
`CFBundleDocumentTypes` — `nca`, `nro`, `nso`, `nsp`, `xci` — which is what
macOS uses to decide whether Ryujinx may open a double‑clicked file. `.pfs0` is
in the scanner and not in the bundle.

The plugin's `GAME_SUFFIXES` is the scanner's six, matched case‑insensitively
because Ryujinx lowercases before it compares, so `GAME.NSP` is a game there and
has to be one here. Two deliberate differences from Ryujinx's own scan:

- **It is not recursive.** Ryujinx enumerates with
  `EnumerationOptions { RecurseSubdirectories = true }`. `host-files` lists one
  directory, never recursively, and never reports a symbolic link — a granted
  folder may not become a door to an ungranted one. So subfolders are not
  imported; allow the folder that holds the files, or make one profile per
  folder.
- **It stops at 256 entries.** `MAX_DIRECTORY_ENTRIES` in `plugin_runtime.rs`
  bounds what *any* plugin may be told about a folder, so a folder with more
  readable entries than that is truncated by name and the rest is invisible.
  The cut leaves a `files-truncated` line in the host journal, which
  Settings → Plugins shows; the contract has no field for telling the plugin.
  See **Limits** below.

## The reference is the file name, in hex

The host resolves the game file itself. A plugin returns an opaque external
reference, and `runner_host::resolve_game_file` matches it against the names it
read out of the granted folder — the plugin never gets a path and the host never
joins anything the plugin said onto one.

That reference has to pass the catalogue's opaque‑id grammar,
`[A-Za-z0-9._\-:]` (`plugin_manifest::valid_opaque_id`). A Switch dump is
conventionally named

```
Super Mario Odyssey [0100000000010000][v0].nsp
```

— spaces and square brackets, neither of which the grammar allows. Before this
lot there was simply no reference a plugin could return for that file, so a
normally named library imported **nothing**. The reference is therefore the
entry name **hex‑encoded**: `53757065722…`. Hex is grammar‑safe, injective (so
it can never name two files) and order‑preserving (so the same encoding works as
a page cursor). `resolve_game_file` learned that one encoding, beside the plain
name it already accepted:

- a plain name, or a name without its final extension, still wins — a library
  that resolved before resolves to the same file now, including when "before"
  meant "ambiguous, refused";
- otherwise, the one entry whose name the reference is the hex of;
- anything matching more than once is refused rather than guessed.

Nothing about the security boundary moves. The reference stays an opaque token,
the file still comes out of the host's own listing of a granted folder,
canonicalised and checked to be inside it, and a decoded `../prod.keys` is just
a string no directory entry is equal to. What widened is *which names a plugin
can say*.

The plugin refuses a reference it could not have issued before it prepares a
launch at all — odd length, a non‑hex digit, bytes that are not UTF‑8, or a name
with no extension Ryujinx opens — which costs no directory scan and keeps
`prepare-launch` inside the interactive budget.

## The cursor is a position, not a name

`discover-page` pages through the host's listing by index: the cursor is the
number of entries already handed out. The listing is sorted by name and bounded,
so a position is stable between two calls of one import, and unlike a name it
cannot outgrow the 256 bytes a cursor is allowed.

What that costs: if a file whose name sorts *earlier* than the cursor appears
between two pages of the same import, this import does not see it. The next
import does — a finished import spends its cursor and starts again from the
beginning, which is how a library that gained files is noticed at all. An
interrupted import resumes from the cursor `commit_resolved_page` persisted
beside the page it describes.

## The title, and the title id

Both come from the file name. Nothing is read out of the file, so there is no
NACP, no icon and no per‑region name — see **Seams** below.

| File name | Title | Card metadata line |
| --- | --- | --- |
| `Super Mario Odyssey [0100000000010000][v0].nsp` | `Super Mario Odyssey` | `Nintendo Switch · 0100000000010000` |
| `The Legend of Zelda - Tears of the Kingdom [0100F2C0115B6000][v0].xci` | `The Legend of Zelda - Tears of the Kingdom` | `Nintendo Switch · 0100F2C0115B6000` |
| `Celeste (USA).nsp` | `Celeste (USA)` | `Nintendo Switch` |
| `hb-launcher.nro` | `hb-launcher` | `Nintendo Switch` |

Square‑bracket groups come off, because that is where a dump puts its title id
and its version. Parentheses stay, because that is where the region usually is
and a region is part of what tells two dumps apart.

A title id is recognised only as a bracketed group of exactly sixteen
hexadecimal digits, and it is upper‑cased — sixteen is specific enough that
`[v0]` or `[US]` cannot be mistaken for one, and two spellings of one id would
search as two games. The v1 `game-candidate` record has no field for an external
title id, so it travels in `platform`, which the host stores as the card's own
metadata line (`Game::metadata`) under the title. That is a decision, not a
convention — see the PR.

## The launch path

```text
Play → LaunchTarget::Runner { runner_id, game_ref, profile_id }
     → profile accepted by the plugin, against the component installed now
     → the folder is the one the user allowed (path and device/inode)
     → the grant is in force
     → prepare-launch, under the interactive budget → LaunchIntent
     → the intent is about this call, and its mode is `default`
     → the host resolves Ryujinx.app → Contents/MacOS/Ryujinx
     → the host resolves the game file inside the granted folder
     → one process, no shell, one argument
```

The application the user picks in the native picker on macOS is `Ryujinx.app`, a
directory. `catalog::resolve_executable` reads `CFBundleExecutable` out of
`Contents/Info.plist` and holds it to one ordinary component of the bundle's own
`MacOS` folder, following no link out of the bundle — the hardening #43's
security review added. Ryujinx's own bundle declares
`CFBundleExecutable = Ryujinx` and `distribution/macos/create_app_bundle.sh`
copies a compiled binary to `Contents/MacOS/Ryujinx`, so what Orivo starts is
that binary and no interpreter.

Ryujinx takes the game as a bare command‑line argument:
`CommandLineState.ParseArguments` assigns any argument that is not a recognised
flag to `LaunchPathArg`, and `Program.cs` starts it when that is set. That is
exactly the one launch mode the v1 contract declares — the application, started
in its own directory, with the resolved game file as its single argument — so
Ryujinx needs no option Orivo cannot express. The flags it *does* accept
(`--fullscreen`, `--root-data-dir`, `--profile`, `--graphics-backend`, …) are
not reachable: the contract has nowhere to declare one and the host tokenises no
strings. See the PR for what widening that would mean.

The working directory is the executable's own folder, so `Contents/MacOS`.
Ryujinx keeps its data under `~/Library/Application Support/Ryujinx` via
`AppDataManager` rather than beside the binary, so this is not the same as
double‑clicking the app (where launchd sets `/`), and nothing observed depends
on it.

## Installing it today

Signing is the registry's business and **who holds Orivo's release key, and
where the index is hosted, is an open question for the project owner**. So
`plugins/ryujinx/package/` ships as `manifest.json` + `component.wasm` with no
`signature.ed25519`, and it installs through the **development channel**: it
shows as unsigned everywhere, it is never updated automatically, and its grants
are tied to its exact bytes rather than to a signature (`plugin_update`'s
`grant_verdict`). Publishing it on the official channel needs three things that
are not code in this repository: the release key, an `plugin_index` entry signed
with it under a domain‑separated context, and a host on the compiled HTTPS
allowlist.

Rebuilding the component:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools --locked --version 1.246.2
plugins/ryujinx/build.sh     # prints component.wasm's sha256 and byte size
```

Paste both into `package/manifest.json`. `cargo test` never runs that script: it
reads the committed `component.wasm` and checks the digest the manifest declares,
so a stale digest fails its own test first
(`ryujinx_plugin::the_committed_component_is_the_artefact_its_manifest_declares`).
Validate and simulate it the way any author would:

```sh
cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- check   plugins/ryujinx/package
cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- simulate plugins/ryujinx/package \
  --request discover-page --grant games=/path/to/your/switch/folder
```

(`validate` additionally wants a `signature.ed25519` beside the manifest, which
the repository deliberately does not carry;
`sdk/orivo-plugin-sdk/tests/ryujinx_plugin.rs` supplies a development one in a
temporary copy.)

## Verifying it by hand, with a real Ryujinx

Nothing in this repository has ever started a real Ryujinx or opened a real
dump — Orivo does not install emulators, and no test here has a game to run.
That check is the user's, and it is four steps:

1. Install Ryujinx yourself (the project's own release), set up your keys and
   firmware **in Ryujinx**, and confirm it opens one game on its own.
2. In Orivo, Settings → Plugins → *Install from file…* and pick
   `plugins/ryujinx/package` packaged as a `.orivo-plugin` archive (or the
   directory, on a development build). It should appear as **Ryujinx 1.0.0**,
   unsigned, ready to configure.
3. *Add an emulator* → choose the Ryujinx plugin → pick `Ryujinx.app` in the
   picker → allow the folder that holds your `.nsp`/`.xci` files. The import
   should report the number of games you can see in that folder, minus
   subfolders and anything past 256 entries.
4. Press Play on one card. Ryujinx should open that game directly, with no
   Ryujinx file dialog in between. If it opens to its own game list instead, the
   argument did not reach it — report the title and the file name.

## Limits worth knowing before you point it at a library

- **256 entries per folder.** A granted folder is listed no further; the rest is
  invisible to the plugin. A 1000‑file folder imports the first 256 by name.
  `docs/performance.md` §7 has the measurement.
- **One folder per profile.** The component asks for one grant slot, `games`,
  because the v1 manifest has no field where a package could declare more than
  one. A second folder means a second profile.
- **No subfolders**, per the listing rule above.
- **No file is read**, so no NACP title, no icon, no version, no DLC or update
  awareness, and a file whose name carries no title id has none.
- **Names longer than 128 bytes** cannot be addressed: hex doubles every byte
  and a reference may be 256. They are counted in the plugin's own journal line.
- **Two files whose names differ only outside the grammar** — `Game A.nsp` and
  `Game-A.nsp` — are distinct references and resolve distinctly; but two files
  with the *same* name in two granted folders of one profile are ambiguous and
  neither is imported, which is `resolve_game_file`'s existing rule.
- **`validate-profile` is nearly vacuous**, because the v1 `runner-profile`
  record is an id and a display name. The plugin refuses an empty one and
  nothing else: the application and the folders are the host's to validate, and
  a verdict that depended on the granted folder would leave every new profile
  permanently rejected, since a profile is created before its first folder is
  allowed and nothing revalidates it afterwards.

## Seams left open

- **Metadata from inside the file.** Reading the NACP out of an `.nsp` would
  give the real per‑language title, the version and the publisher, and it is
  deliberately not done: it needs `read-file` on the user's dumps, which is the
  one capability this plugin's whole shape is built to avoid. If it is ever
  wanted, it belongs to a `metadata` plugin with its own grant and its own
  consent screen, not to the runner.
- **Updates and DLC.** `.nsp` update and DLC packages are indistinguishable from
  base games by name, so today they import as their own cards. Telling them
  apart means reading the file (above) or a naming convention this plugin does
  not guess at.
- **A second emulator.** The plan asks for one, measured, before a second
  integration; `docs/TODOS.md` also names an Apple‑Silicon‑native Switch
  emulator, whose library format and launch contract would have to be verifiable
  the same way this one's were.
- **Android.** `runner_commands`' pickers refuse on Android, so a third‑party
  runner cannot be configured there yet; Ryujinx has no Android build of its own
  either.

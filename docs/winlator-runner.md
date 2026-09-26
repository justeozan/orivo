# The Winlator runner

Winlator runs Windows games on Android: Box64 translates x86‑64 to ARM, Wine
provides the Windows API, and DXVK/Turnip put Direct3D on Vulkan. It is a
separate application, not a library, so Orivo's runner for it is a **hand‑off**,
not a process it owns.

Like Wine‑Staging, this is a first‑party **native Rust adapter** that honours the
`runner@1` contract in [`wit/orivo-plugin.wit`](../wit/orivo-plugin.wit) — 
`validate-profile`, `discover-page`, `prepare-launch` → `LaunchIntent` — without
going through Wasm. It lives in
[`src-tauri/src/winlator_runner.rs`](../src-tauri/src/winlator_runner.rs) and is
compiled everywhere but can only launch on `target_os = "android"`.

## What Winlator actually exposes to another app

Everything below was read from each project's own manifest and source, not from
a guide.

| Distribution | Package | Launch activity | Exported? |
| --- | --- | --- | --- |
| Official (brunodev85), 11.2 | `com.winlator` | `com.winlator.XServerDisplayActivity` | **No** (`android:exported="false"`) |
| Cmod lineage (Winlator Cmod, Ludashi, Bionic‑CMOD) | `com.winlator.cmod` | `com.winlator.cmod.XServerDisplayActivity` | **Yes** |

In the official build the only exported activity is `com.winlator.MainActivity`,
and its exported entry points (`container_id` + `start_path`) open the container
*file manager* — there is no way to start a game. [PR #79][pr79], which would
have exported `XServerDisplayActivity` and added a `command` extra, was **closed
without being merged**. Orivo therefore models `Official` as a profile it can
name and refuses the launch with a sentence, rather than sending an intent the
platform would reject.

Orivo also deliberately does **not** want that PR's `command` extra: Winlator
would run it as `cmd /c "<command>"`. A free‑form command string is the one thing
the runner contract forbids.

### The Cmod contract

`com.winlator.cmod.XServerDisplayActivity` reads exactly three extras that matter
here:

- `container_id` — `int`. Absent or `0` makes Winlator read the container out of
  the shortcut file itself.
- `shortcut_path` — `String`. Absolute path to a `.desktop` file.
- `shortcut_name` — `String`. Optional; Winlator parses `Name=` out of the file
  when it is missing.

Cmod's own **“Export shortcut to frontend”** action is what makes this reachable:
it copies a container shortcut into shared storage (by default
`/storage/emulated/0/Download/Winlator/Frontend`), rewrites its `container_id=`
line, and writes a `FRONTEND_INSTRUCTIONS.txt` and a `metadata.pegasus.txt` next
to it that document the launch verbatim:

```
am start -n com.winlator.cmod/com.winlator.cmod.XServerDisplayActivity -e shortcut_path {file_path}
```

Orivo sends that same explicit component from Rust through JNI. It never shells
out, so `am` is not involved.

## What Orivo owns, and what it cannot

A Winlator **container** — and the Wine prefix inside it — lives in
`<Winlator files>/imagefs/...`, in that app's private storage. Orivo can neither
create, read, nor validate it. So a Winlator profile is a *reference*:

```text
WinlatorProfile { id, display_name, distribution, container_id?, shortcut_directories, enabled }
```

| Orivo can verify | Orivo cannot verify |
| --- | --- |
| the granted directory exists and is a directory | that the container id exists |
| the shortcut is inside the grant, after canonicalisation | that the container's Wine prefix is healthy |
| the shortcut's bytes still hash to what was imported | that Winlator's drive mapping reaches the game |
| that the shortcut is a `.desktop` file | that the game's `.exe` is still there |
| that the distribution has an exported launch activity | that Winlator is installed — until the intent is sent |

That last row is the design's quiet upside: an **explicit component** bypasses
intent filters, so Orivo needs no `<queries>` entry in `AndroidManifest.xml` to
see the package. A missing Winlator surfaces as `ActivityNotFoundException`, a
build that unexported the activity as `SecurityException`, and both are turned
into a readable sentence. This matters because `src-tauri/gen/` is not tracked:
a manifest change could not be delivered by a commit anyway.

There are consequently **no graphics options** on a Winlator profile. Every
per‑game setting — the DX wrapper, the Box64 preset, the screen size, the env
vars — belongs to Winlator's shortcut. Inventing Orivo-side options would only
be able to lie.

## Where the game files live

The `.exe` stays wherever the user put it on the device, normally on shared
storage. Winlator reaches it through its container's **mapped drives**: the Cmod
default is `D:` → the public `Download` directory and `E:` → Winlator's own
`storage` directory. Orivo never resolves that mapping, never rewrites a DOS
path, and never passes an executable path: the shortcut Winlator wrote already
contains the drive-letter path it wants.

Orivo's own scope is therefore over **shortcuts**, not executables — and it is
the same grant model as Wine's: the profile carries a list of granted
directories, a shortcut outside them is refused with `ShortcutOutsideScope`,
symlinks are never followed, and every candidate is canonicalised again before
it receives an opaque reference.

## The launch path

```text
WebView → game_id
  → LaunchTarget::Runner { runner_id: "com.orivo.winlator", profile_id, game_ref }
  → WinlatorProfile + WinlatorShortcutInventoryEntry (host-private)
  → WinlatorLaunchIntent { runner_id, profile_id, game_ref, mode: ExportedShortcut }
  → prepare_winlator_launch: canonicalise, scope-check, re-hash the shortcut
  → AndroidIntent { package, activity, flags, extras: [Int|Text with &'static str keys] }
  → re-hash once more, immediately before the hand-off
  → JNI: Intent().setComponent(ComponentName).addFlags().putExtra()… startActivity()
```

`AndroidIntent` is closed: extra **keys** are `&'static str` from a compile-time
table, and extra **values** are either a bounded integer or a string the host
produced from a file it had just verified. Nothing from the WebView reaches it.
Building one is pure, which is why `cargo test` asserts the exact component,
flags and extras on macOS, with no emulator.

The flags are `FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_CLEAR_TASK |
FLAG_ACTIVITY_CLEAR_TOP` — Cmod's documented frontend set minus
`FLAG_ACTIVITY_NO_HISTORY`, which finishes the activity as soon as it stops and
would end a running game the moment the player switched away.

The shortcut is hashed twice on purpose: once while preparing the intent, and
once immediately before it leaves the process. Winlator rewrites an exported
shortcut whenever the user re-exports it, and the new file can point at a
different executable or container, so a changed shortcut is refused until a
deliberate reimport has updated Orivo's private inventory.

## How a Winlator game gets into the library

On Android, startup adopts whatever Winlator has already exported: if the default
frontend directory exists, its `.desktop` files are scanned, scope-checked,
hashed, and turned into cards behind one managed profile
(`orivo-auto-winlator`), provisioned without a wizard — the same shape as the
managed default Wine profile. A pass that finds nothing new does not rewrite
`catalog.json`, and a profile the user disabled is left alone.

Nothing is added to the catalog schema version: `winlator_profiles` and
`winlator_inventory` are optional arrays, so a `catalog.json` written before this
change loads unchanged.

## Seams left for the “Add an emulator” flow

- Choosing a distribution and a shortcut directory by hand, instead of the
  managed default: `WinlatorProfile` already carries both.
- Enabling, disabling and deleting a profile: `enabled` is honoured on every
  path; a removal helper is the missing piece.
- A visible import with progress and cancellation:
  `scan_winlator_shortcuts` already takes an `AtomicBool` and a progress
  callback, and `page_winlator_inventory` already pages a snapshot.
- Re-running adoption without a restart.

[pr79]: https://github.com/brunodev85/winlator/pull/79

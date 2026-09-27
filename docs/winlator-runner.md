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
  → WinlatorShortcutSource: a readable directory, or the granted document tree
  → prepare_winlator_launch: resolve, scope-check, re-hash the shortcut
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

## Reading the export folder: the storage access framework

Sending the intent needs no permission. *Finding* the shortcut does, and by
pathname Orivo cannot have it.

Measured on the emulator (Android 17 / API 37), as Orivo's own uid:

```
$ adb shell run-as io.orivo.desktop ls /storage/emulated/0/Download/Winlator/Frontend
ls: /storage/emulated/0/Download/Winlator/Frontend: Permission denied
```

That is scoped storage doing its job: a `.desktop` file is not media, so on
API 30+ an app opens it by path only with `MANAGE_EXTERNAL_STORAGE`. Orivo's
manifest asks for `INTERNET` and nothing else — and `src-tauri/gen/` is not
tracked, so a manifest permission is not something a commit here could deliver
even if it were wanted.

The answer is **`ACTION_OPEN_DOCUMENT_TREE`**: the user points at the folder
once, Orivo takes a *persistable* read permission on it, and every read after
that goes through a `ContentResolver`. No manifest change, a grant the user can
see and revoke in the system settings, and nothing to ask for again after a
restart. It is also the grant the "Add an emulator" flow would have created
anyway.

### The one hard part: a document is not a path

Winlator's `shortcut_path` extra is a **file path**. It opens that file itself,
with its own permissions — a `content://` URI would be a file it cannot open. So
Orivo has to be able to say, without guessing, when a document it can read *is* a
file at a path Winlator can open.

Exactly one provider allows the claim to be made:

| Provider | Identifier | Is it a path? |
| --- | --- | --- |
| `com.android.externalstorage.documents`, `primary:` | `primary:Download/Winlator/Frontend/Celeste.desktop` | **Yes** — `<getExternalStorageDirectory()>/Download/Winlator/Frontend/Celeste.desktop` |
| `com.android.externalstorage.documents`, `1234-ABCD:` | a removable volume | No — the mount point would have to be inferred |
| `com.android.providers.downloads.documents` | `msf:42` | No — a row, not a file |
| any cloud provider | opaque | No |

So the rule, in [`src-tauri/src/winlator_saf.rs`](../src-tauri/src/winlator_saf.rs),
is narrow on purpose: the authority must be primary external storage, the
document identifier must start with `primary:`, the shared volume's root is asked
for (`Environment.getExternalStorageDirectory()`) rather than assumed to be
`/storage/emulated/0`, and every other case is refused with a sentence —
*"Orivo can only read a folder in this device's own storage."* — instead of a
guessed path. The mapping is a bijection under those conditions, which is why the
catalog stores no second copy of it: the inventory keeps the path, and the
document identifier is recomputed from it when the file has to be read again.

A profile therefore holds **both halves** of the grant: `shortcut_directories`,
the folder Winlator opens and the scope every check already used, and
`shortcut_trees`, the tree URIs that are only *how Orivo reads* it. Neither
replaces the other.

### Two grants, one pipeline

The scanner did not fork. `WinlatorShortcutSource` has two implementations — a
readable directory, and a granted document tree — and everything above it is
written once: the bounded, cancellable walk, the 64 KiB cap, the scope check, the
SHA-256 fingerprint, the intent. The filesystem source canonicalises a pathname
and opens it with `O_NOFOLLOW`; the SAF source resolves a document identifier and
opens a stream. A device with a plainly readable folder still uses the first, and
so does every test that does not need a provider.

A document tree is checked **twice**, because its two halves can disagree:

- as an identifier, which has to sit under the granted tree — a provider is free
  to answer a listing with any row it likes, and a row that does not resolve to a
  child of the folder being listed is dropped rather than followed;
- as a path, which has to sit under the directory that tree stands for — an
  identifier containing `..` looks perfectly inside the tree to the provider and
  would leave the folder on the filesystem.

Both are exercised on a host against a fake provider that returns those rows on
purpose, along with an oversized document, an unreadable one, a directory row that
would make the walk loop, and a document rewritten between preparing an intent
and sending it.

### Receiving the folder: one Kotlin class, in this repository

JNI covers nearly all of Android from Rust — sending the intent, querying a
`ContentResolver`, opening a document. It does not cover *receiving an activity
result*: `onActivityResult` lands on a Java class, and none can be conjured at
runtime.

| Option | Verdict |
| --- | --- |
| Pure JNI | The picker's result never arrives. A `java.lang.reflect.Proxy` for Tauri's `ActivityResultCallback` still needs a Java `InvocationHandler`; loading a dex at runtime is worse than a build-time class in every way. |
| A Kotlin file in `src-tauri/gen/android` | Untracked and regenerated. It could not be delivered by a commit. |
| A third-party crate | None found that takes a *persistable* tree permission, which is the whole point; and it would be a new dependency for sixty lines of Kotlin. |
| **A local Tauri plugin** | Chosen. [`tauri-plugin-orivo-saf/`](../tauri-plugin-orivo-saf) is a path dependency of this workspace, so its Android library project is tracked, and `tauri-build` wires it into the generated project on every build. |

That plugin does three things and stops: it starts the chooser, takes the
persistable read permission on the folder that comes back — which only the
process that received the grant may do — and hands Rust the tree URI as a string.
It declares no permission, exposes no command to the WebView, and adds no Cargo
dependency the repository did not already have.

## How a Winlator game gets into the library

Adoption reads whatever Winlator has already exported — the connected folder if
there is one, the default export path otherwise — scope-checks and hashes each
`.desktop` file, and turns it into a card behind one managed profile
(`orivo-auto-winlator`), provisioned without a wizard, the same shape as the
managed default Wine profile. A pass that finds nothing new does not rewrite
`catalog.json`, and a profile the user disabled is left alone.

**It does not run at startup.** It used to, inside `AppState::new`, under a
comment claiming the first paint never waited on it — true only while the folder
was unreachable, and false the moment a grant made it readable. It is now a
background pass started by the first finished page load: bounded by the scanner's
own limits, cancellable, and silent unless it changed something, in which case it
says so and the library reloads rather than waiting for a restart. On a desktop it
returns before doing anything at all.

Connecting a different folder overtakes a pass that is already walking the old
one — both write the same managed profile — and it keeps the profile's identity
while dropping the inventory entries that fell outside the new grant: those could
never be launched again, and the catalog's own scope check would refuse the write.

Nothing is added to the catalog schema version: `winlator_profiles`,
`winlator_inventory` and the `shortcut_trees` a profile now carries are all
optional, so a `catalog.json` written before any of this loads unchanged.

## What was verified on a device

On the `Orivo_Test` emulator (arm64, Android 17 / API 37) with **Winlator Cmod
v13.1.1** installed from its own GitHub release, and a container created through
Winlator's own UI.

The folder is unreachable by pathname, as Orivo's own uid:

```
$ adb shell run-as io.orivo.desktop ls /storage/emulated/0/Download/Winlator/Frontend
ls: /storage/emulated/0/Download/Winlator/Frontend: Permission denied
```

Connecting it from the Sources menu opens the system chooser; picking
`Download/Winlator/Frontend` and allowing it makes Orivo read the shortcut —
through the provider, by document, never by path:

```
MediaProvider: Open with lower FS for
  /storage/emulated/0/Download/Winlator/Frontend/Orivo Test Game.desktop. Uid: 10111
```

The card appears in the library, and pressing Play hands the game over. This is
Winlator's own log, and it is the link that was missing before — Winlator
**reads the extras**, resolves the container from them, and starts Box64 and
Wine for that shortcut:

```
ActivityTaskManager: START u0 {flg=0x14008000 xflg=0x4
  cmp=com.winlator.cmod/.XServerDisplayActivity (has extras)}
  with LAUNCH_SINGLE_TASK from uid 10230 (io.orivo.desktop)
XServerDisplayActivity: Shortcut Path: /storage/emulated/0/Download/Winlator/Frontend/Orivo Test Game.desktop
XServerDisplayActivity: Container ID from Intent: 1
XServerDisplayActivity: Intent Extras: Bundle[{shortcut_name=Orivo Test Game,
  shortcut_path=/storage/emulated/0/Download/Winlator/Frontend/Orivo Test Game.desktop,
  container_id=1}]
ProcessHelper: cmd: .../usr/bin/box64 wine explorer /desktop=shell,1280x720
  winhandler.exe /dir D: "Orivo-Test-Game.exe"
```

`0x14008000` is exactly `NEW_TASK | CLEAR_TASK | CLEAR_TOP`.

Two more things were watched on the device rather than only in a test. Editing
the shortcut after it was adopted made Play refuse it — *"This Winlator shortcut
changed. Export it again from Winlator so Orivo can pick it up."* — with no
intent sent at all. And killing Orivo and starting it again re-read the same
folder with no second chooser: the grant is persisted, and the background pass
picked the edited shortcut back up.

**Where it stops.** The Wine session itself never finishes booting in this
emulator: Winlator sits on *"Starting up…"* whether it is started from Orivo or
from Winlator's own container list, which is nested emulation meeting Box64 and
Vulkan, not anything Orivo does. So no game is *displayed* here. Everything up to
and including Winlator spawning Box64 and Wine for the shortcut Orivo adopted is
what the log above shows.

## Seams left for the “Add an emulator” flow

- Choosing a distribution and *several* folders by hand, instead of one managed
  default: `WinlatorProfile` already carries both, and connecting one folder — in
  the Sources menu, on Android only — is already wired end to end.
- Enabling, disabling and deleting a profile: `enabled` is honoured on every
  path; a removal helper is the missing piece.
- A visible import with progress and cancellation:
  `scan_winlator_shortcuts` already takes an `AtomicBool` and a progress
  callback, and `page_winlator_inventory` already pages a snapshot.
- Re-running adoption without a restart.

[pr79]: https://github.com/brunodev85/winlator/pull/79

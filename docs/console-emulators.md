# Console emulators on Android

After Windows through Winlator, the consoles. On Android the emulators already
exist and the player already has them, so Orivo's runner for each of them is a
**hand-off**, not a process it owns — the same shape as
[the Winlator runner](winlator-runner.md), for the same reason.

Like Wine-Staging and Winlator these are first-party **native Rust adapters**
that honour the `runner@1` contract in [`wit/orivo-plugin.wit`](../wit/orivo-plugin.wit)
— `validate-profile`, `discover-page`, `prepare-launch` → `LaunchIntent` — without
going through Wasm. They live in
[`src-tauri/src/console_runner.rs`](../src-tauri/src/console_runner.rs), are
compiled everywhere and can only launch on `target_os = "android"`.

## What each emulator actually exposes to another app

Everything below was read from each project's own manifest and source, and then
from the APK that was installed on the emulator to verify it.

| Emulator | Packages | Launch surface | Exported? | The ROM arrives as |
| --- | --- | --- | --- | --- |
| **RetroArch** 1.22.2 | `com.retroarch.aarch64`, `com.retroarch`, `com.retroarch.ra32` | `com.retroarch.browser.retroactivity.RetroActivityFuture` | **Yes** | `ROM` string extra: a **file path** |
| **PPSSPP** 1.20.4 | `org.ppsspp.ppsspp`, `org.ppsspp.ppssppgold` | `org.ppsspp.ppsspp.PpssppActivity` | **Yes** | `ACTION_VIEW` + `intent.getData()`: a **content URI** |

### RetroArch

`RetroActivityFuture` is `android:exported="true"` in
`pkg/android/phoenix/AndroidManifest.xml`, and the native side reads exactly two
extras that matter here, in `frontend/drivers/platform_unix.c`:

- `ROM` → `args->content_path`, the game to auto-start;
- `LIBRETRO` → `args->libretro_path`, the core to load.

Both are **paths**. `LIBRETRO` names a shared library RetroArch will `dlopen`,
which is why nothing about it is ever composed from outside the host: the console
is a closed enum, the core file name comes from a compile-time table, and the
directory is a constant. RetroArch keeps the cores it downloaded in
`ApplicationInfo.dataDir/cores`, and for a primary-user install `dataDir` is
`/data/user/0/<package>` — which RetroArch's own `CoreSideloadActivity` prints
when it refuses to run:

```
Error: Destination directory doesn't exist (/data/user/0/com.retroarch.aarch64/cores)
```

Orivo cannot *ask* for that directory. `PackageManager.getApplicationInfo` is
filtered on API 30+ unless the package is visible, which needs a `<queries>`
manifest entry `src-tauri/gen/` could not deliver — and the platform says so out
loud the moment Orivo addresses RetroArch at all:

```
AppsFilter: interaction: PackageSetting{io.orivo.desktop/10235}
  -> PackageSetting{com.retroarch.aarch64/10233} BLOCKED
```

The *launch* needs no visibility, because an explicit component bypasses intent
filters entirely. So the core path is composed, and a device where that is wrong
fails legibly — RetroArch says it cannot load the core — rather than silently.
A secondary Android user, whose `dataDir` is `/data/user/<id>/…`, is the known
limit; a profile carrying its own core directory is the seam for E3's
"Add an emulator" flow.

The consoles Orivo names, and the libretro core each one resolves to:

| Console | Core | Extensions |
| --- | --- | --- |
| NES | `fceumm_libretro_android.so` | `nes` `fds` `unf` `unif` |
| SNES | `snes9x_libretro_android.so` | `smc` `sfc` `swc` `fig` |
| Game Boy / Color | `gambatte_libretro_android.so` | `gb` `gbc` |
| Game Boy Advance | `mgba_libretro_android.so` | `gba` |
| Mega Drive / Master System | `genesis_plus_gx_libretro_android.so` | `md` `smd` `gen` `sms` `gg` |

No extension is claimed by two of them, which is what lets **one connect cover a
whole emulator**: the user grants one folder, and each file lands under the
console its extension names. An archive is deliberately not on any list — Orivo
would then be offering a file whose contents it never looked at — and neither is
a generic extension such as `bin`, because every console claims it and the folder
is shared storage.

### PPSSPP

`PpssppActivity` is `android:exported="true"` and declares an `ACTION_VIEW`
filter with the `file` **and `content`** schemes. `PpssppActivity.parseIntent`
reads `intent.getData()` and passes the URI straight to the native side, which
has its own content-URI file layer. So PPSSPP is handed **the very document Orivo
read**, with `FLAG_GRANT_READ_URI_PERMISSION` on the intent — no path, no folder,
and nothing about where shared storage is mounted. It logs what it received:

```
PpssppActivity: Found Shortcut Parameter in data, passing on:
  "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FRoms
    /document/primary%3ADownload%2FRoms%2FPSP%2FEBOOT.pbp"
```

The URI is never assembled by Orivo: `DocumentsContract.buildDocumentUriUsingTree`
is the platform's own builder and the JNI layer calls it, so nothing here depends
on the shape of a `content://` string.

### The ones that were left out, and why

- **Dolphin** (GameCube/Wii). `.activities.EmulationActivity` is
  `android:exported="false"`; only `MainActivity`, `TvMainActivity` and an
  `AppLinkActivity` for deep links are exported, and none of them starts a game
  from an intent. Like brunodev85's Winlator, there is no intent Orivo could send
  that the platform would not reject.
- **melonDS Android** and **Flycast**. Both *do* expose a launch surface —
  melonDS through an explicit `…LAUNCH_ROM` action on an exported
  `EmulatorActivity`, Flycast through `ACTION_VIEW` on its launcher alias — and
  both need BIOS images Orivo cannot obtain and this lot could not verify a launch
  without. They are the obvious next two, and the table above is where they go.
- Anything whose entry point is a **command string**. The runner contract forbids
  it; it is the same line Winlator's PR #79 crossed.

## What Orivo owns, and what it cannot

A profile is a *reference*, not an installation:

```text
ConsoleEmulatorProfile { id, display_name, emulator, system,
                         rom_directories, rom_trees, enabled }
```

| Orivo can verify | Orivo cannot verify |
| --- | --- |
| the granted folder is one it may read at all | that the emulator is installed — until the intent is sent |
| the ROM is inside the grant, after canonicalisation | that the core, the BIOS or the save directory are set up |
| the ROM still hashes to what was imported | that the game will run |
| the file is one this console uses | what the file *is* — Orivo never parses a ROM |

There are consequently **no emulator options** on a profile. The renderer, the
controls, the save states and the cores belong to the emulator, and inventing
Orivo-side settings would only be able to lie.

## Passing the ROM

Two shapes, and the emulator chooses which:

- **A content URI**, for an emulator that takes one. This is strictly better:
  the emulator gets read access to exactly one file, for one launch, and no path
  is involved.
- **A path**, for an emulator that opens the file itself. That needs the same
  narrow rule Winlator needs — the authority must be primary external storage,
  the document identifier must start with `primary:`, the volume root is *asked*
  for rather than assumed — and every other case is refused with a sentence
  instead of a guessed path. The rule itself is not written twice: it is
  [`winlator_saf::DocumentTreeGrant`](../src-tauri/src/winlator_saf.rs), reused,
  because a second copy of a security rule is a second place for it to be wrong.

A grant on the volume root, on a shared drop folder (`Download`, `DCIM`,
`Documents`, …) or anywhere under `Android/` is refused, and that check runs on
**every use** of a grant rather than only when it was picked. A folder *inside*
one of them — `Download/Roms` — is exactly right.

## Is this still the file you added?

A `.desktop` file is read whole and hashed at every launch. A ROM cannot be: a
disc image is measured in gigabytes, and a launch that pulled 1.5 GB through a
`ContentResolver` before starting would not be a launch.

So a ROM's identity is its **byte length together with a digest**:

- up to 32 MiB — every cartridge ever pressed, with room to spare — the digest is
  of the **whole file**, and the answer is exact;
- above that, it is of the first 1 MiB, where every disc format keeps its header
  and its table of contents.

The two are never comparable: the fingerprint's prefix says which one it is
(`sha256:` or `sha256-head:`), and the length is inside the digest either way.
What the partial form catches is a file replaced by a different game, a file
truncated, and a file appended to. What it does not catch is an image of the same
length whose bytes changed only past the head — a patch applied in place to a
game the user already approved. Reading the whole image before every launch is the
price of catching that, and it is not one a player pressing Play should pay.

Asking for the head *first* is also what keeps the cost honest: the first read
reveals the true length, so a 6 MB file costs two reads and a 1.5 GB image costs
one. A provider that will not say how long a document is is refused rather than
fingerprinted without it — a digest of a prefix with no length attached is the
same for a file and for that file with anything appended.

## How a console game gets into the library

The same model as Winlator's, corrected in the two places its counter-review
found — and those corrections landed first, on Winlator itself, because this lot
reuses them.

```text
Sources ▸ RetroArch games / PPSSPP games
  → the folder, picked once (or reviewed again, with no chooser)
  → a bounded, cancellable scan, with no catalog lock held
  → what was found, in the menu: the name, the console, the file, the folder
  → "Add this game" / "Not now"
  → only the chosen references become cards
```

**A file in a shared folder is a question, never an answer.** Any app can create
one in `Download/<subfolder>/` through MediaStore with no permission at all, so
finding a file is not a reason to make it a card. And a ROM's title *is* its file
name, which means two files can claim the same game: the confirmation therefore
carries the file, the folders between the connected one and it, and a sentence
when the name is already a game in the library's or another file's. Every part of
it is set through `textContent` — it came out of a file on shared storage.

Between that question and the answer, the file can be replaced. The import
re-reads it and refuses it unless it still hashes to what the preview showed, and
says so rather than importing whatever is there now under the name that was
confirmed. The same check runs again immediately before the intent leaves the
process.

Cards live behind managed profiles (`orivo-auto-retroarch-nes`,
`orivo-auto-ppsspp-psp`, …), one per emulator and console, provisioned without a
wizard. Connecting a different folder keeps the profiles — and so the card ids —
while dropping the inventory entries that fell outside the new grant: those could
never be launched again, and the catalog's own scope check would refuse the write.

Nothing is added to the catalog schema version: `console_profiles` and
`console_inventory` are optional, so a `catalog.json` written before any of this
loads unchanged.

## What was verified on a device

On the `Orivo_Test` emulator (arm64, Android 17 / API 37), with **RetroArch
1.22.2 (AArch64)** and **PPSSPP 1.20.4** installed from their own official
downloads, and three freely-licensed homebrew ROMs plus one PSP homebrew. No APK
and no ROM is committed to this repository.

The folder is unreachable by pathname, as Orivo's own uid:

```
$ adb shell run-as io.orivo.desktop ls /sdcard/Download/Roms
ls: /sdcard/Download/Roms: Permission denied
```

Connecting `Download/Roms` from the Sources menu — Android's own picker refuses
`Download` itself — makes Orivo read every file through the provider, by
document, never by path:

```
MediaProvider: Open with lower FS for …/Roms/240p Test Suite.nes. Uid: 10111
```

Nothing is imported: the menu lists what it found, each row naming the console,
the file and the folder, and the library stays empty until **Add these 3 games**.
Then Play hands the game over, and RetroArch reads both extras:

```
ActivityTaskManager: START u0 {flg=0x10000000 xflg=0x4
  cmp=com.retroarch.aarch64/com.retroarch.browser.retroactivity.RetroActivityFuture
  (has extras)} with LAUNCH_SINGLE_INSTANCE from uid 10235 (io.orivo.desktop)
RetroArch: [ENV] Libretro path: "/data/user/0/com.retroarch.aarch64/cores/fceumm_libretro_android.so".
RetroArch: [ENV] Auto-start game "/storage/emulated/0/Download/Roms/240p Test Suite.nes".
```

`0x10000000` is exactly `FLAG_ACTIVITY_NEW_TASK`, and **the game plays**: the
240p Test Suite's first screen, emulated by fceumm, started from a card in Orivo.

PPSSPP receives the other shape — `ACTION_VIEW`, the document URI, and
`0x10000001` = `NEW_TASK | GRANT_READ_URI_PERMISSION` — and reads the file through
the grant: it decodes and displays the homebrew's own embedded artwork. Its loader
then refuses that particular PBP (*"Could not find executable umd0:/EBOOT.pbp"*).
**No PSP title is run here.** Whether that is PPSSPP's PBP-over-content-URI path
or this build of the homebrew was not chased down: it is on the other side of the
hand-off. Everything up to and including PPSSPP reading Orivo's document is in the
log above.

Two more things were watched rather than only tested. Replacing an imported ROM
made Play refuse it — *"This file changed since you added it. Add it again so
Orivo knows what it is."* — with no intent sent. And a second file dropped into
the connected folder was offered **by name, and only it**, on the next review: the
ones already imported were not offered again, and the grant survived Orivo being
killed and restarted with no second chooser.

Device housekeeping: both emulators, Orivo and every file pushed were removed
afterwards; the AVD is back to how it was found, and `emulator-5554` was never
touched.

## Seams left for the "Add an emulator" flow

- Several folders, and a profile per folder rather than one managed set.
- A core directory on a RetroArch profile, for a device where `/data/user/0` is
  not where that emulator's data lives.
- Enabling, disabling and deleting a profile: `enabled` is honoured on every path;
  a removal helper is the missing piece.
- A background pass that says what is waiting, the way Winlator's does.
- A visible import with progress and cancellation: `scan_roms` already takes an
  `AtomicBool` and a progress callback.

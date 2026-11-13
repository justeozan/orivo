# The iOS sideload edition

The App Store app, under another identifier, distributed outside the App
Store: every release carries an unsigned `Orivo_<version>_ios-sideload.ipa`
that AltStore, SideStore or Sideloadly sign with the player's own Apple ID,
and orivo.evey.run serves the AltStore source that keeps it up to date.

It exists to be a channel. Whatever iOS lets an app do once App Review is out
of the way — a runtime that needs JIT, code the app downloads — needs a way to
reach players, and this is it. Until something like that lands, the edition is
the App Store app and nothing more.

## What it changes today: nothing but its identity

Every iOS restriction in the code was checked against "is this the App Store's
rule, or iOS's?":

| Restriction (`src-tauri/src/platform.rs`) | Whose | In the sideload edition |
| --- | --- | --- |
| No plugin from the registry (`downloadable_plugins`) | the App Store's guideline 2.5.2, **and** iOS: Cranelift needs memory it may both write and execute, which only a debugger-attached app gets | still off: lifting the guideline leaves the JIT |
| No runner, no streaming (`local_runners`) | iOS: an app spawns no process, and the apps it would hand a game to register no URL | still off |
| Install asks where to get a game (`store_choices`) | iOS: no store client to ask | unchanged |
| No update check or button (`self_updates`) | iOS: Tauri's updater has no installer there | still off: the AltStore source delivers new versions, so the app never offers to update itself |

So the first real difference will come with work that needs it — detecting
JIT when the player has enabled it, or a runtime — and that work will add the
switch, rather than this edition adding one nobody reads.

## Identity

`src-tauri/tauri.ios-sideload.conf.json` sets the identifier to
`run.evey.orivo.sideload`. A different app to iOS, so it installs next to the
App Store version, keeps a data directory (and so a library) of its own, and
can never be taken for the App Store record's build. Sideloaders rewrite the
identifier again when they sign — a free Apple ID cannot register one another
team owns — but the source has to name the one inside the package.

## Building

```sh
rustup target add aarch64-apple-ios
rustup component add llvm-tools
scripts/build-ios-sideload.sh            # → target/ios-sideload/
```

The script generates `src-tauri/gen/apple` when it is missing, points it at
the edition's identifier (`scripts/sync-ios-project.sh <config>`), builds a
release for devices with `tauri ios build --no-sign`, and writes two files:

- `Orivo_<version>_ios-sideload.ipa`, the app, unsigned;
- `Orivo_<version>_ios-sideload.json`, read back from that package: bundle id,
  versions, minimum iOS, size, sha256, and the entitlements and privacy keys it
  carries. AltStore refuses an app whose source does not list every one of
  those, so they are taken from the build, never written by hand.

Under Xcode 27 the first release link stops on three undefined swift-rs
symbols ("Xcode 27 and swift-rs" in [`app-store.md`](app-store.md)). The
script recognises that failure, promotes the symbols in Tauri's archive and
builds again; an Xcode that does not need it never takes that path.

The package is unsigned on purpose: whoever installs it signs it. It declares
no entitlement and no privacy permission today; one added later is picked up by
the facts file on the next build.

## Releasing

The `ios-sideload` job of `.github/workflows/release.yml` runs the script on a
macOS runner and attaches both files to the draft that `prepare-draft`
creates. It is not a release gate yet — the note on the job says
why and how to make it one.

## The source

orivo.evey.run (the `orivo-site` repository) answers:

| | |
| --- | --- |
| `/altstore.json` | the AltStore / SideStore source |
| `/sideload` | the page that explains the edition and adds the source |
| `/download/ios-sideload` | the latest `.ipa`, for Sideloadly and the rest |

The Worker builds the source from the newest published release that carries
both files, so a release whose iOS job failed is skipped instead of emptying
it. Before the first one, the source is valid and has no app in it.

"Add to AltStore" is `https://altstore.io/source/orivo.evey.run/altstore.json`;
"Add to SideStore" is `sidestore://source?url=https://orivo.evey.run/altstore.json`.

## For players

A free Apple ID signs an app for seven days and holds three at a time: the
edition has to be refreshed weekly (SideStore does it on the phone; AltStore
needs its AltServer). The first install needs a computer either way. No JIT,
no Developer Mode: nothing in the app asks for either yet.

## Keeping the editions apart

The App Store app does not mention the sideload edition, link to it or
recommend it: an App Store app that sends people to sideloading is one App
Review can refuse. The website and the release notes are where the edition is
found.

# App Store

What the App Store Connect record for Orivo declares, and why. The answers are
derived from the code, not from intent: if what the app sends off the device
changes, this file, [`PRIVACY.md`](../PRIVACY.md) and the record have to move
together.

| | |
| --- | --- |
| Record | **Orivo Games**, Apple ID `6813145265`, bundle ID `run.evey.orivo` |
| Versions | iOS and macOS 0.3.11 — the app's own version, which a build has to match |
| Devices | iPhone and iPad (`TARGETED_DEVICE_FAMILY = "1,2"`) |
| Listing language | English (U.S.) |
| Privacy policy | <https://github.com/justeozan/orivo/blob/main/PRIVACY.md> |
| Support | <https://github.com/justeozan/orivo/issues> |
| Release | manual, after approval |

## Building for the record

iOS builds are `run.evey.orivo`: `src-tauri/tauri.ios.conf.json` overrides the
identifier for iOS, as `tauri.android.conf.json` does for Android, whose Google
Play record has the same package ([`google-play.md`](google-play.md)). The
desktop stays `io.orivo.desktop`, which names the data directory existing
installs live under.

`src-tauri/gen/apple` is generated and gitignored, and `tauri ios init` gives
it Tauri's logo as the app icon. After an init, before any iOS build:

```sh
scripts/sync-ios-project.sh   # identifier + Orivo's icon (run-ios-simulator.sh runs it)
```

The sideload edition ([`ios-sideload.md`](ios-sideload.md)) is another app,
`run.evey.orivo.sideload`, built unsigned by the release workflow: nothing in
this record refers to it, and the App Store app never mentions it.

The icon set in `src-tauri/icons/ios` is RGB on purpose. `tauri icon` writes
RGBA even from an opaque source, and App Store Connect rejects the channel
itself (ITMS-90717); `src/ios-app-store.test.ts` fails if one comes back.

### Building and uploading

Signing is automatic: an App Store Connect API key lets Xcode create the
`run.evey.orivo` App Store profile itself, against the team's Apple
Distribution certificate, which has to be in an unlocked keychain —
`errSecInternalComponent` from the export step means codesign could not reach
its key.

```sh
rustup component add llvm-tools          # once, see below
scripts/sync-ios-project.sh
APPLE_DEVELOPMENT_TEAM=<team id> APPLE_API_KEY=<key id> APPLE_API_ISSUER=<issuer id> \
APPLE_API_KEY_PATH=<path to AuthKey_<key id>.p8> \
  pnpm tauri ios build --export-method app-store-connect
xcrun altool --upload-package <the exported .ipa> --type ios \
  --api-key <key id> --api-issuer <issuer id>
```

The build number (`CFBundleVersion`) is the app version unless told
otherwise, and App Store Connect refuses a second upload under the same one.
To send another build of a version already uploaded, give it a higher number
for that build only, without touching the committed config:
`pnpm tauri ios build … --config '{"bundle":{"iOS":{"bundleVersion":"2"}}}'`.

Export compliance is answered in `src-tauri/Info.ios.plist`
(`ITSAppUsesNonExemptEncryption`), so a build needs no questionnaire.

**Xcode 27 and swift-rs.** A release Swift build under Xcode 27 makes every
`@_cdecl` function local, and the Rust side can no longer link to Tauri's
Swift half. swift-rs 1.0.8 promotes the archive's own functions back to
global with `llvm-objcopy` (hence `llvm-tools`), but not the three of its own
runtime that every archive embeds a copy of — `release_object`,
`retain_object`, `string_from_bytes` — so the first release build stops on
"Undefined symbols" for those three. Promote them in exactly one archive,
Tauri's (a second global copy is a duplicate symbol), make Cargo bundle the
`tauri` crate again without re-running its build script, and build again:

```sh
T=target/aarch64-apple-ios/release
A=$(ls $T/build/tauri-*/out/swift-rs/Tauri/release/libTauri.a)
"$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-objcopy" \
  --globalize-symbol=_release_object --globalize-symbol=_retain_object \
  --globalize-symbol=_string_from_bytes "$A"
rm -rf $(dirname $T/.fingerprint/tauri-*/lib-tauri)   # the lib unit only, not its build script
```

## The Mac App Store edition

The Mac build on the record is not the DMG. It is built with the cargo
feature `app-store`, runs in the App Sandbox and goes through the same review
as iOS, so `platform.rs` turns off what the review or the sandbox refuses:
downloaded plugins (no code from anywhere but the App Store, and no JIT
entitlement), the runners (a sandboxed process cannot run Wine or a store's
own downloader) and the updater, which is not even registered. Quiky is not
compiled in. Steam, Epic, GOG and the others still start games through their
own apps' URL schemes.

The sandbox's `$HOME` is the app's container, so Steam's and Epic's folders,
where the installed games are written, are out of reach. Settings › Libraries
offers to open each one once with the system folder picker, already on the
right folder; the grant is kept as a read-only security-scoped bookmark and
reopened at every launch (`folder_access.rs`). Libraries themselves come from
the accounts and need no folder.

### Streaming through Moonlight

The Moonlight plugin is a downloaded runner, so the edition cannot install it,
and the sandbox would not let it work anyway: `NSWorkspace` drops the
arguments of an app it opens, and a process the app spawns inherits the
sandbox, so a Moonlight started that way reads its pairing from Orivo's
container and finds none. Streaming is built in instead, through the one door
the sandbox leaves: a script in `~/Library/Application Scripts/run.evey.orivo/`,
which macOS runs outside the sandbox through `NSUserUnixTask`
(`moonlight_bridge.rs`).

- **One click to allow.** The app may read that folder but not write it.
  Settings › Plugins › Moonlight opens it in the system's folder panel, the
  player clicks Open, and the grant lets Orivo write `orivo-moonlight` there.
  Deleting the file takes the permission back.
- **The script starts Moonlight and nothing else**, with `list`, `pair` or
  `stream`, and the host checks every argument before asking it
  (`validate_arguments`). Moonlight keeps its own pairing and settings, so a
  player who already streams with it goes straight to the games.
- **The PC's games become `stream` cards** (`GameSource::Stream`), started with
  `moonlight stream <PC> <game>`. No new entitlement: the edition's own are
  enough.

Measured on 2026-11-17 from a sandboxed build with these entitlements: `list`
answers in 1.3–1.7 s and a stream starts. The alternatives were worse for the
player: a `shortcuts://` link brings Shortcuts to the front at every launch,
Apple Events to Terminal leave a window open and need a temporary-exception
entitlement, and a Moonlight spawned inside the sandbox needs its own pairing
and `network.server`. Moonlight has no URL scheme and no AppleScript
dictionary, so nothing can talk to it directly.

App Review will not have Moonlight or a gaming PC. The review notes should say
so: the Moonlight card explains itself, and the rest of the app does not depend
on it.

Signing needs the team's Apple Distribution certificate, a *Mac Installer
Distribution* certificate ("3rd Party Mac Developer Installer") and a Mac App
Store profile for `run.evey.orivo`, all three created through the API key.
`src-tauri/tauri.appstore.conf.json`, `Entitlements.appstore.plist` and
`Info.appstore.plist` hold the rest:

```sh
VITE_SENTRY_DSN=… SENTRY_AUTH_TOKEN=… \
ORIVO_MAS_PROFILE=<path to the .provisionprofile> scripts/build-mac-app-store.sh
xcrun altool --upload-package <the .pkg it prints> --type macos \
  --api-key <key id> --api-issuer <issuer id>
```

It builds for Apple Silicon by default, which App Store Connect accepts only
with macOS 12 as the minimum (hence `minimumSystemVersion` in its config); `ORIVO_MAS_TARGET=universal-apple-darwin`
adds Intel, with `rustup target add x86_64-apple-darwin` first. A build
signed for the store does not open outside it: test it through TestFlight,
or re-sign a copy ad hoc with the same entitlements to watch the sandbox.

## Listing

- **Subtitle**: All your games in one place
- **Categories**: Entertainment, then Utilities. Not Games: Orivo is not a
  game, and guideline 2.3.5 lets Apple move an app that says otherwise.
- **Keywords**: no store or game names. Third-party brands in keywords are
  rejected under guideline 2.3.7; the description names the stores instead.
- **Content rights**: the app shows third-party content (store artwork,
  descriptions and logos from the stores' public catalogs).
- **Price**: free. **Availability**: every territory except mainland China,
  where an app needs an ICP filing number before it can be listed.

## On iPhone and iPad

An iOS build runs no PC game and may download no code, and two things follow
(`src-tauri/src/platform.rs` states them for the WebView):

- **Plugins.** Nothing comes from the registry: `entry_runs_here` offers no
  entry, so the catalogue, Install, Update and the background updater have
  nothing to fetch, and the file picker was already off on mobile. Settings ›
  Plugins keeps only what is built in and works there, the wallpaper search.
  Guideline 2.5.2 refuses an app that downloads code, and Cranelift could not
  compile a component on iOS anyway.
- **Install.** Steam cannot be asked to install anything (`steam://` has no
  handler on iOS). The button asks the host where the game can be got instead
  (`src-tauri/src/store_choices.rs`): its own iOS version when the App Store
  sells one by a studio Steam names, then its Steam page, which is always
  there. One answer opens directly; two are offered beside the button.
- **Playport.** On a device where Playport is installed, the PC version comes
  first in that menu, and a GOG or Epic Games game, which has no other answer
  there, gets an Install of its own ([`playport.md`](playport.md)). Playport
  runs Windows games on the device itself; Orivo only asks whether it is there
  (`canOpenURL`, with `playport` in `LSApplicationQueriesSchemes`) and brings
  it to the front. Playport is sideloaded, so Orivo never names, links or
  recommends it where it is not installed: without it, the menu is what it was.

Streaming from a computer and emulation are not offered on iOS. The Moonlight
app for iOS registers no URL scheme, so no other app can start a stream in it,
and Orivo has no source of ROMs there.

Nor does Orivo update itself there: the App Store installs new versions, so the
host answers `self_updates: false`, nothing looks for a release on GitHub, and
Settings › About has no Updates row. Android builds do the same, for Google
Play ([`google-play.md`](google-play.md)).

## Age rating: 12+

Orivo itself shows nothing objectionable, but every screen is built from the
art of the games in the library, and that art includes weapons and fantasy and
realistic violence. Those three, plus horror themes, are answered
*infrequent*. Everything else is *none* or *no*: no advertising, no gambling,
no loot boxes, no chat, no user-generated content, no health topics, no
unrestricted web access.

## App Privacy

There is no public API for this form: App Store Connect › App Privacy.

Everything hinges on one build setting. `src/sentry.ts` only starts Sentry
when `VITE_SENTRY_DSN` is set **when the frontend is built**, and the release
workflow sets it from a repository secret.

**Built without the DSN**: answer *Data Not Collected*.

**Built with the DSN**: *Data collected*, none of it used for tracking.

| Apple category | Type | Where it comes from | Purpose | Linked to the user |
| --- | --- | --- | --- | --- |
| Diagnostics | Crash Data | Sentry error reports | App Functionality | No |
| Diagnostics | Performance Data | Sentry traces, 10 % of sessions | App Functionality | No |
| Usage Data | Product Interaction | Sentry Session Replay (clicks and scrolling, text and media masked), 10 % of sessions and every session with an error | App Functionality | No |
| Contact Info | Name | optional field of the Feedback form | App Functionality | Yes |
| Contact Info | Email Address | optional field of the Feedback form | App Functionality | Yes |
| User Content | Customer Support | the Feedback message and its optional screenshot | App Functionality | Yes |

`sendDefaultPii: false` keeps names and IP addresses off the error reports.

Not "collected" in Apple's sense:

- Store sign-ins (Steam, Epic Games, GOG, Ubisoft Connect, Xbox, Microsoft
  Store, Instant Gaming): the user signs in on the store's own page, and the
  credential stays in the device keychain. No Orivo server is involved.
- The library, preferences, wishlist and the IGDB / SteamGridDB / Google keys:
  stored on the device only.
- Requests to the stores' public catalogs and CDNs: no personal data is sent.
- The request for Playport's public compatibility list (playport.dev), made
  only on a device where Playport is installed: nothing is sent with it.

No advertising or analytics SDK, no advertising identifier.

## Review notes

The reviewer needs no store account: Settings › Appearance › *Beta & debug* ›
**Show demo games** fills the library with the bundled demo games. The notes
on the record say so, in English, and the review contact is filled in there.

## Screenshots

Six per device, all landscape: iPhone 6.9" (2868 × 1320), iPad 13"
(2752 × 2064) and Mac (2880 × 1800). They are the real frontend, captured at
Apple's sizes with a library built from the official Steam and IGDB art of
twelve games, so the pipeline lives outside this repository with that art.

On iPhone and iPad no game is launchable and the one on stage comes from
Steam, because that is what the iOS build shows: a PC game cannot start on
iOS, so its button reads *Install*, not *Play*. Only the Mac screenshots show
*Play*. Screenshots have to match the app (guideline 2.3).

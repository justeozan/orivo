# Google Play

What Orivo's Google Play record declares, and why. As with
[`app-store.md`](app-store.md), the answers are derived from the code: if what
the Android build sends off the device changes, this file,
[`PRIVACY.md`](../PRIVACY.md) and the record have to move together.

| | |
| --- | --- |
| Record | **Orivo**, package `run.evey.orivo`, created October 14, 2026 |
| App or game | App, category Entertainment |
| Listing language | English (United States) |
| Price | Free |
| Privacy policy | <https://github.com/justeozan/orivo/blob/main/PRIVACY.md> |
| Contact | support@orivo.evey.run, <https://orivo.evey.run> |
| App signing | Play App Signing, with a key Google generated and keeps |

## The package

Android builds are `run.evey.orivo`, the package the Play record was created
with and the identifier the iOS build already has.
`src-tauri/tauri.android.conf.json` overrides the identifier for Android, as
`tauri.ios.conf.json` does for iOS; the desktop keeps `io.orivo.desktop`.

The package is written in four places, and `src/android-play.test.ts` holds
them together:
- Tauri's identifier, where wry writes its Kotlin glue and the JNI symbols the
  library exports;
- Gradle's `namespace`, where `MainActivity`, which extends that glue, is
  looked for;
- `applicationId`, the package Play and the device see;
- `MainActivity.kt` itself.

A drift between the first two still builds, and crashes at launch.

0.3.7 to 0.3.11 shipped their APKs as `io.orivo.desktop`. Android treats that
as another app: an install of one keeps working with its own library, and a
later version installs beside it rather than over it.

## Signing

Play signs what it distributes with an app signing key it generated and
keeps, so nobody can lose or leak it. What Play checks on a bundle is the
**upload key**: the release job signs with it (`ANDROID_UPLOAD_KEYSTORE_*`
secrets), and a lost or leaked one can be replaced through Play support. Its
certificate SHA-256 is
`48:A4:59:81:C9:3C:9A:CD:B9:E5:66:A2:BD:77:7D:AF:91:60:88:92:F4:3F:6E:34:09:59:23:EE:FD:9F:CD:DC`.

Android only replaces an installed app with a build that has the same package
and the same signing key. So the APK a GitHub release offers is not built by
the job: it is the universal APK Play generates from the bundle and signs
with the app signing key, and an install from GitHub and an install from Play
update each other. A build signed with the upload key never leaves CI.

That only works with Play's **automatic protection** off. When it is on, Play
rewrites every APK it generates: `com.pairip.licensecheck` checks the
installer at launch, and an install that did not come from the Play Store
closes itself and opens Play instead. The release script refuses such an APK
rather than publish one that cannot open. Protection is set in Play Console ›
App integrity, and applies to the bundles Play receives after the change.

The key that signed 0.3.7 to 0.3.11 (`ANDROID_KEYSTORE_*`) signs nothing any
more. It is what one of those installs would need to be updated in place,
which is why the secrets are kept.

## The release job

For a release that is not a pre-release, the `android` job:

1. builds the bundle and signs it with the upload key;
2. trades its GitHub OIDC token for a Google Play access token through
   Workload Identity Federation, so no key to the Play record is stored
   anywhere. The repository variables `GOOGLE_PLAY_WORKLOAD_IDENTITY_PROVIDER`
   and `GOOGLE_PLAY_SERVICE_ACCOUNT` name the provider and the service account
   it impersonates;
3. puts the bundle on the internal testing track
   (`scripts/google-play-release.mjs`). The release is completed, or a draft
   while the app has never been published, since a draft is all Play accepts
   then;
4. downloads the universal APK Play generated, and attaches it to the release
   as `Orivo_<version>_android.apk`, beside the bundle.

Re-running a release whose bundle Play already has is safe: Play takes a
version code once, so the script fetches the APK Play generated the first time.

A pre-release gets the bundle only. Tauri derives the version code from major,
minor and patch, so a pre-release and its release share one, and Play takes a
version code once.

Moving a build from internal testing to a closed test or to production is
done in the Play Console.

The service account has, on this app only, the Play Console permissions
"Release to testing tracks", "Edit and delete draft apps" and "View app
information (read-only)". Its identity provider admits OIDC tokens from this
repository's tags and `main` only.

## The build Play receives

The release job's `app-universal-release.aab`, as it is. Checked on 0.3.7:

- target SDK 36, which Play requires of new apps and updates since August 31,
  2026; minimum SDK 24.
- arm64 only (`--target aarch64`). Play requires 64-bit code, not 32-bit.
- `liborivo_lib.so` is built for 16 KB pages: the job links with
  `-z max-page-size=16384`, so its `LOAD` segments are aligned to `0x4000`,
  and the APK passes `zipalign -c -P 16`. Play has refused bundles that are not
  since November 2025.
- One permission, `INTERNET`, plus the
  `DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` AndroidX declares for itself.
  Package visibility is declared in `tauri-plugin-orivo-saf`: the emulator,
  Winlator and Moonlight packages by name, and the launcher intent a home
  screen declares, which is how the phone's own games are listed. Not
  `QUERY_ALL_PACKAGES`, which Play would ask to justify.
- The version code comes from the version: major × 1,000,000 + minor × 1,000 +
  patch, so 0.3.7 is 3007. Play accepts a version code once, so a build that
  must be replaced ships as the next version.

`aapt2 dump badging` on the APK from the same run shows the manifest;
`llvm-readelf -l` on the `.so` shows the alignment.

## What the Android build does not do

Play's policies and its review apply to what Orivo is in four places, and each
has an answer in the code:

- **It does not update itself.** An app from Play may update itself only
  through Play (Device and Network Abuse). `PlatformFeatures::self_updates` is
  false on every mobile build: Orivo never looks for a release there, and
  Settings › About has no Updates row. The release manifest has no Android
  entry anyway; a sideloaded APK is updated by installing the next one.
- **It downloads no native code.** Registry plugins are WebAssembly components
  run by Wasmtime in a sandbox with no network and no process. They fall under
  the same policy's exception for "code that runs in a virtual machine or an
  interpreter where either provides indirect access to Android APIs". Nothing
  Orivo downloads is a dex, a JAR or a `.so`. The Epic games it downloads
  (`epic_android.rs`) are Windows programs it never loads: Winlator, a separate
  app the player installed, runs them.
- **Its buttons do not fail by design.** No Steam client on a phone takes a
  download request, so Install on a Steam game asks the host where the game
  can be got, as on iOS (`store_choices`): on Android that is the game's Steam
  page, since Google Play has no public search for an Android version. An Epic
  game is downloaded by Orivo itself and played in Winlator. A game from
  another PC store shows as unavailable rather than offering a button that
  could only fail.
- **It ships no game and no emulator.** Console games start in RetroArch or
  PPSSPP and Windows games in Winlator, apps the player installs themselves
  ([`console-emulators.md`](console-emulators.md),
  [`winlator-runner.md`](winlator-runner.md)). Orivo hands them a file the
  player picked through the system's folder chooser, or an Epic game it
  downloaded from Epic for the account that owns it. It downloads no ROM and
  links to no source of one.

## Listing

- **Title**: Orivo. The App Store record is named "Orivo Games".
- **Short description**: All your games in one place: Steam, Epic Games, GOG,
  Ubisoft Connect and Xbox.
- **Category**: Entertainment. Not a game, for the same reason as on the App
  Store.
- **Full description**: the App Store's, with what runs on Android in place of
  what does not on iOS, and the limit said plainly: a game from Steam or
  another PC store stays in the library and is played on the PC.

## Graphics

- **Icon**: 512 × 512, the iOS 1024 × 1024 set scaled down. It is full-bleed
  on purpose: Play applies its own mask.
- **Feature graphic**: 1024 × 500, the phone showing the library, centred,
  because Play crops the sides in some placements.
- **Screenshots**: six per device, all landscape and 16:9. Play refuses a
  screenshot whose long side is more than twice its short side, so a phone's
  own 20:9 capture cannot be used: phone 1920 × 1080, 7-inch tablet
  2048 × 1152, 10-inch tablet 2560 × 1440. As on the App Store they are the
  real frontend, rendered with the library of twelve games the App Store
  captures use, and the pipeline lives outside this repository with that art.
  The game on stage is a Steam game showing Install, because Android cannot
  start a game from a PC store either.

## App content

The forms the Play Developer API does not reach.

**Privacy policy**: the URL above.

**Ads**: none.

**App access**: some functionality is restricted, and the instructions say how
to see it without an account. A library fills up once a store is connected,
which needs the reviewer's own store account; Settings › Appearance ›
*Beta & debug* › **Show demo games** fills it with the bundled demo games
instead. Orivo has no account of its own.

**Content rating**: Orivo shows nothing objectionable itself, but every screen
is built from the art of the games in the library, which includes violence and
horror themes. That is the App Store's 12+, and the questionnaire is answered
the same way. Orivo is not a game: it shows store catalogues and their prices,
and buys nothing itself.

**Target audience**: 13 and over, not appealing to children, for the same
reason. Choosing an age group under 13 would bring the Families policy, which
the art of a library cannot meet.

**Data safety**: like App Privacy, it hinges on `VITE_SENTRY_DSN`, which Vite
inlines when the frontend is built. The release job's Android step does not
pass it, so the bundle Play receives answers **no data collected, no data
shared**:

- Store sign-ins go from the device to the store, at the user's request, and
  stay in the Android Keystore. No Orivo server is involved.
- Requests to the stores' public catalogues, IGDB, SteamGridDB and Google image
  search carry game names or store identifiers, not personal data.
- The library, preferences, wishlist and keys stay on the device.

If the DSN is ever passed to the Android build, these become collected, none
shared, all encrypted in transit:

| Play category | Type | From | Purpose | Optional |
| --- | --- | --- | --- | --- |
| App info and performance | Crash logs | Sentry error reports | App functionality | No |
| App info and performance | Diagnostics | Sentry traces, 10 % of sessions | App functionality | No |
| Personal info | Name, Email address | the Feedback form | App functionality | Yes |
| Messages | Other in-app messages | the Feedback message | App functionality | Yes |
| Photos and videos | Photos | the Feedback screenshot | App functionality | Yes |

**Advertising ID**: not used. The manifest does not ask for `AD_ID`.

**Government apps, financial features, health, news**: none apply. There is no
account to delete, because there is no account.

## From test to production

A personal developer account created after November 13, 2023 reaches
production only after a closed test with at least 12 testers who stayed opted
in for the last 14 days; an organization account does not have to. Internal
testing, up to 100 testers and with no review, is where a new bundle lands
first.

A draft app, one never published, takes releases through the API only as
drafts: the rollout itself is pressed in the Console. The API cannot create
the app, and none of the forms above has an endpoint, apart from Data safety,
which takes the CSV the Console exports.

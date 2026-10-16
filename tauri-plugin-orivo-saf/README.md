# `tauri-plugin-orivo-saf`

Orivo reaches Android from Rust through JNI, and that covers almost everything —
sending an intent, querying a `ContentResolver`, reading a document. This crate
carries the Kotlin for the few things that need a Java class to land on, and
nothing else:

- **The folder chooser.** `ACTION_OPEN_DOCUMENT_TREE` answers through
  `onActivityResult`, which arrives on a Java class; no amount of JNI can
  conjure one at runtime. `pickDocumentTree` starts the picker, takes the
  persistable read permission on the tree the user chose (only the process that
  received the grant may), and hands the tree URI back as a string.
- **The games installed on the phone.** `listInstalledGames` lists the apps
  with a launcher entry that Android files under its game category, and draws
  three pictures from each icon — the icon, a portrait cover and a landscape
  wallpaper — into the app cache's `media` folder, the one directory the
  WebView may read. Only names and file names leave Kotlin;
  `src-tauri/src/android_games.rs` decides what becomes a card.
- **Starting another app.** `launchPackage` starts an installed game through
  the launch intent its own package declares, and `startStream` hands a game to
  Moonlight (or its Artemis fork) through Moonlight's exported
  `ShortcutTrampoline`: the PC by the name Moonlight shows, the game by the
  name the PC's streaming host lists (`src-tauri/src/moonlight_android.rs`).

Everything after that — what a tree may be, which documents are shortcuts, how
they are bounded, hashed and turned into a path Winlator can open — is host logic
in `src-tauri/src/winlator_saf.rs`, where it is testable without a device.

The manifest's `<queries>` names the packages Orivo asks about (the console
emulators, Winlator Cmod, Moonlight and Artemis) and the launcher intent, the
narrowest declaration that lets an app see which games a phone has.

It lives in this repository as a crate, rather than as a file in
`src-tauri/gen/android`, because `tauri-build` wires a plugin's Android library
project into the generated project on every build, manifest included.

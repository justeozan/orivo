# `tauri-plugin-orivo-saf`

Orivo reaches Android from Rust through JNI, and that covers almost everything —
sending an intent, querying a `ContentResolver`, reading a document. It does not
cover **receiving an activity result**: `onActivityResult` arrives on a Java
class, and no amount of JNI can conjure one at runtime.

`ACTION_OPEN_DOCUMENT_TREE` returns its folder exactly there. So this crate
exists to carry the ~60 lines of Kotlin that own that callback, and nothing else:

- it starts the picker,
- it takes the persistable read permission on the tree the user chose, so the
  grant survives a restart,
- it hands the tree URI back to Rust as a string.

Everything after that — what a tree may be, which documents are shortcuts, how
they are bounded, hashed and turned into a path Winlator can open — is host logic
in `src-tauri/src/winlator_saf.rs`, where it is testable without a device.

It lives in this repository, and not in `src-tauri/gen/android`, because
`gen/` is generated and untracked: a Kotlin file committed there would not
survive a regeneration and could not be delivered by a commit at all. A plugin's
Android library project *is* trackable, and `tauri-build` wires it into the
generated project on every build.

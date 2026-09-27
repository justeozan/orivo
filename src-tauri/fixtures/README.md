# Plugin host fixtures

Four committed WebAssembly components, and the seams the host exposes for testing
against them. `cargo test` reads the `.wasm` files and checks their SHA-256; it
never builds them, so a fresh clone needs no WebAssembly target and no component
tool.

| Artefact | Source | What it is for |
| --- | --- | --- |
| `orivo-runner-fixture.wasm` | `runner-fixture/` (Rust + `wit-bindgen`) | The reference third-party runner. Implements `runner-plugin` properly, and misbehaves on request. |
| `wasi-import.wasm` | `wasi-import.wat` (component text) | The smallest package that must be refused: its only import is WASI. |
| `memory64.wasm` | `memory64.wat` (component text) | A core module with a 64-bit linear memory. The engine turns that feature off, so this must be refused. |
| `composed-memories.wasm` | `composed-memories.wat` (component text) | Two composed components, each with its own memory, and a string crossing between them. Makes Wasmtime synthesise an adapter module importing both. |

## Rebuilding

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools --locked --version 1.246.2
src-tauri/fixtures/runner-fixture/build.sh
```

The script prints each artefact's digest and size. Paste them into the
`FIXTURE_SHA256`, `WASI_IMPORT_SHA256`, `MEMORY64_SHA256` and
`COMPOSED_MEMORIES_SHA256` constants in `src-tauri/src/plugin_runtime.rs`; the
registry tests compute the runner digest themselves. A stale digest fails its own test first, which is
the point: every other sandbox assertion is only meaningful if the artefact under
test is the one the source describes.

`wasm-tools` is pinned to the release whose component encoding matches the
wasmtime the host depends on.

## Asking the fixture to misbehave

`runner.prepare-launch`, `runner.validate-profile` and `runner.discover-page` all
branch on the opaque identifier the host passes in, so one small component covers
the nominal path and every refusal:

| Selector | What the component does |
| --- | --- |
| `fixture:ok` | returns the launch intent the host asked for |
| `fixture:spin` | never returns — fuel, deadline and cancellation |
| `fixture:grow` | allocates until the memory ceiling refuses it |
| `fixture:recurse` | fills the wasm stack with real call frames |
| `fixture:shadow-stack` | fills Rust's own stack, the one inside linear memory |
| `fixture:trap` | executes `unreachable` |
| `fixture:bad-mode` | returns a launch mode the host does not recognise |
| `fixture:bad-target` | answers about a different profile and game |
| `fixture:bad-runner` | claims to be preparing another runner's launch |
| `fixture:bad-id` | returns a game reference that is really a path |
| `fixture:deny` | asks for a directory grant it was never given |
| `fixture:escape` | reads `../` out of the folder it *was* given |
| `fixture:fail` | returns a plain WIT error |
| `fixture:chatty` | earns a refusal, swallows it, then floods the journal |
| `fixture:shout` | logs messages far larger than the host will keep |
| `fixture:megashout` | hands one host call a four-megabyte argument |
| `fixture:nag` | earns the same refusal four hundred times |
| `fixture:census` | reports the listing back, entry by entry, through the journal |
| `fixture:churn` | spends the whole call inside host calls, computing almost nothing |
| `fixture:bury` | earns a refusal, then churns until the ring should have lost it |
| `fixture:read-NAME` | reads `NAME.rom` by name, whatever the host planted there |

`discover-page` reads its selector from the *profile id* rather than a game
reference, because what it is asked to get wrong is the page it hands back:

| Selector | What the page looks like |
| --- | --- |
| `fixture:dup` | the same external reference twice |
| `fixture:overfill` | more rows than the host asked for, all distinct |
| `fixture:huge` | a title no view model would take |
| `fixture:bad-cursor` | a cursor that is really a path |
| `fixture:loop-cursor` | the cursor it was handed, unchanged |
| `fixture:done-cursor` | `complete`, and somewhere to continue from |

Two of the stack selectors look alike and are not. `fixture:recurse` takes no
address of a local, so its frames are wasm locals and land on the native stack
`max_wasm_stack` bounds; `fixture:shadow-stack` takes the address of a 512-byte
array, which forces Rust to put each frame in linear memory and run out of a
different region entirely, long before that ceiling. Which one stops the
component is visible in the journal, because the host records the trap it got.

The component reads exactly one directory grant, named `fixture-games`, and lists
`*.rom` entries whose contents are the game titles. A grant for any other id, or
a name that is not a single entry of that folder, is the host's to refuse.

`fixture:read-NAME` exists because the listing is not the only way in: it asks for
an entry by name whether or not `list-directory` offered it. That is how a test
points the component at something it planted in the granted folder — a symbolic
link out of it, a FIFO, a file larger than the host will read — and checks that
the *read* refuses it rather than trusting the listing to have filtered it.

## Seams in the host

These exist so a suite can be adversarial without reaching into private state.

- **Injectable limits.** `PluginLimits` and `SchedulerLimits` are plain values;
  `PluginRuntime::with_all_limits` takes both. Shrinking a deadline to two ticks
  or a memory ceiling to 8 MiB needs no feature flag.
- **Controlled epoch.** `EpochMode::Manual` stops the runtime from spawning its
  tick thread, and `PluginRuntime::tick_epoch` advances the epoch by hand. The
  deadline is counted in ticks rather than read off a clock, so a test decides
  exactly when a component runs out of time.
- **Cancellation.** `PluginRuntime::invoke` takes the cancel token directly, and
  `JobContext::cancel_token` hands a scheduled job the same one. Both paths reach
  a call already inside Wasmtime.
- **Grants without a UI.** `PluginGrants::declared_only` is a plugin between
  install and configuration; `PluginGrants::resolve` maps opaque grant ids onto
  real directories, so a test supplies its own temporary folder. `resolve` *opens*
  each of them: a grant is a descriptor, not a path, so a test can swap the folder
  or a parent afterwards and watch the grant hold.
- **Grants that outlive the value.** A descriptor pins a folder for as long as a
  `PluginGrants` lives, and nothing longer. Whatever persists grants stores a
  *path*, and a path is answered by whatever is at it on the next start, so the
  approval also records `PluginGrants::directory_identity` — the folder's device
  and inode, as two plain integers — and `PluginGrants::resolve_pinned` refuses
  anything else. It checks *after* opening, so the answer is the one actually
  given. The identity follows the folder rather than the path, so a library the
  user moved still resolves; a different folder at the same path does not.
  Whoever owns grant storage has to keep both halves next to the path.
- **Journal, in three rings.** `entries()` is the host's decisions, `traces()` is
  per-call bookkeeping, and `plugin_messages()` is the plugin's own text. Only the
  first is a refusal, and it is the one nothing a plugin does in a loop can write
  to: traffic goes to `traces()`, and a decision reached twice under the same
  correlation is counted in `repeats` rather than appended. That is what makes
  asserting on a refusal meaningful after two hundred and fifty-six host calls.
  The decision ring also carries `trap`, which is how a test tells a guest that
  filled the wasm stack from one that filled its own stack inside linear memory.
- **Cost.** A successful `invoke` returns `InvocationCost`: instantiation, call,
  fuel burned, host calls made and bytes read. The last two are what fuel does not
  measure — a page that reads a hundred files burns barely more fuel than one that
  reads two — and they are how "resumes without rescanning the library" becomes a
  number rather than a claim.
- **Worker stacks.** Guest code runs on scheduler workers and nowhere else, sized
  from `PLUGIN_THREAD_STACK_BYTES`. `PluginRuntime::invoke` is private for that
  reason: a thread smaller than `max_wasm_stack` plus host headroom turns a guest
  stack overflow into an abort, so the only public door is `submit`.
- **Compilation cannot unwind.** `without_unwinding` turns a panic inside
  Wasmtime's translator — it `expect`s its own invariants — into
  `PluginRuntimeError::InvalidComponent`. It is generic over the work, so a test
  can hand it a panic instead of a component.

### Gaps, and where they are

Three of these are open, and this section exists so nobody reads the code as
closing them.

- **Windows does not pin a grant.** `openat` has an equivalent there —
  `NtCreateFile` with a directory handle as `RootDirectory`, which is what `std`'s
  unstable `fs::Dir` uses (`std/src/sys/fs/windows/dir.rs:95-103`) — so the
  swapped-parent and junction redirection *is* closable; it is not closed. Neither
  is the folder identity, which needs `GetFileInformationByHandle`. Both need code
  that cannot be type-checked on the machine this was written on: `cargo check
  --target x86_64-pc-windows-msvc` stops in `ring`'s build script for want of a
  Windows C toolchain, and Orivo's CI only `cargo check`s Windows. A stored
  identity is therefore *refused* on Windows rather than waved through, which is
  the direction that fails safe.
- **The hard-link rule does not fire on Windows.** `EntryFacts` cannot count links
  there without the same handle query, so it reports one name and the rule never
  triggers. `FolderTrust` is unknown there too, for want of a DACL read.
- **A listing enumerates through a path.** Names come from `read_dir`; every fact
  comes from the handle. An attacker who swaps a parent can hide entries, never
  reveal one. Enumerating from the descriptor needs `fdopendir`/`readdir`.

A listing cut at 256 entries leaves a `files-truncated` decision behind. The
contract has no field for it — see the ABI note in the PR that added this — so the
journal is the only place a short library currently comes with a reason.

Two of the refusal fixtures are there because of limits the host got wrong once:
`memory64.wasm` for the feature that reaches Wasmtime's one
`memory_grow_failed`-without-`memory_growing` path, and `composed-memories.wasm`
for the adapter modules that made turning multi-memory off a panic rather than a
refusal.

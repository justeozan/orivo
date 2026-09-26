# Plugin host fixtures

Two committed WebAssembly components, and the seams the host exposes for testing
against them. `cargo test` reads the `.wasm` files and checks their SHA-256; it
never builds them, so a fresh clone needs no WebAssembly target and no component
tool.

| Artefact | Source | What it is for |
| --- | --- | --- |
| `orivo-runner-fixture.wasm` | `runner-fixture/` (Rust + `wit-bindgen`) | The reference third-party runner. Implements `runner-plugin` properly, and misbehaves on request. |
| `wasi-import.wasm` | `wasi-import.wat` (component text) | The smallest package that must be refused: its only import is WASI. |

## Rebuilding

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools --locked --version 1.246.2
src-tauri/fixtures/runner-fixture/build.sh
```

The script prints each artefact's digest and size. Paste the digests into the
`FIXTURE_SHA256` and `WASI_IMPORT_SHA256` constants in
`src-tauri/src/plugin_runtime.rs`, and the runner digest into the registry tests
that build a package around it. A stale digest fails its own test first, which is
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
| `fixture:recurse` | recurses until a stack ceiling refuses it |
| `fixture:bad-mode` | returns a launch mode the host does not recognise |
| `fixture:bad-target` | answers about a different profile and game |
| `fixture:bad-runner` | claims to be preparing another runner's launch |
| `fixture:bad-id` | returns a game reference that is really a path |
| `fixture:deny` | asks for a directory grant it was never given |
| `fixture:escape` | reads `../` out of the folder it *was* given |
| `fixture:fail` | returns a plain WIT error |
| `fixture:chatty` | earns a refusal, swallows it, then floods the journal |
| `fixture:read-NAME` | reads `NAME.rom` by name, whatever the host planted there |

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
  real directories, so a test supplies its own temporary folder.
- **Journal.** `PluginRuntime::journal().entries()` returns the host's decisions,
  which is how a refusal is asserted on rather than inferred from an absence.
- **Cost.** A successful `invoke` returns `InvocationCost`: instantiation, call
  and fuel actually burned.
- **Journal, in two halves.** `entries()` is the host's decisions and
  `plugin_messages()` is the plugin's own text. They are separate rings so a
  component cannot bury a refusal under its own logging, and a test can assert on
  either without the other interfering.
- **Worker stacks.** Guest code runs on scheduler workers and nowhere else, sized
  from `PLUGIN_THREAD_STACK_BYTES`. `PluginRuntime::invoke` is private for that
  reason: a thread smaller than `max_wasm_stack` plus host headroom turns a guest
  stack overflow into an abort, so the only public door is `submit`.

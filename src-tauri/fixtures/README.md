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
| `fixture:bad-mode` | returns a launch mode the host does not recognise |
| `fixture:bad-target` | answers about a different profile and game |
| `fixture:bad-id` | returns a game reference that is really a path |
| `fixture:deny` | asks for a directory grant it was never given |
| `fixture:escape` | reads `../` out of the folder it *was* given |
| `fixture:fail` | returns a plain WIT error |

The component reads exactly one directory grant, named `fixture-games`, and lists
`*.rom` entries whose contents are the game titles. A grant for any other id, or
a name that is not a single entry of that folder, is the host's to refuse.

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

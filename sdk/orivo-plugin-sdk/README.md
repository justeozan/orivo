# orivo-plugin-sdk

A plugin author's loop before there is a marketplace to submit to. Three
commands, none of which reimplement a rule Orivo already enforces:

- `validate` calls `orivo_lib::plugin_manifest::PluginManifest::validate`
  (and, for a full directory, `validate_plugin_package`) directly. The
  manifest rules live in exactly one place; this binary links against it as a
  library rather than keeping a second copy that could drift.
- `check` calls `PluginRuntime::inspect_contract` and
  `PluginRuntime::verify_runner` — the exact pre-install gate the registry
  runs before offering an installed runner for configuration.
- `simulate` calls `PluginRuntime::submit` and waits on the returned job, the
  same door every other caller in Orivo uses to run guest code. Grants are
  resolved with `PluginGrants::resolve` against directories you point it at on
  your own disk, standing in for a persisted grant the "Add an emulator" flow
  will eventually create.

See `wit/README.md` for the full author walkthrough (template → build →
validate → simulate → package) and `templates/runner-plugin-starter` for a
minimal, non-adversarial `runner-plugin` component to start from.

## Usage

```sh
cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- validate <manifest.json | package-dir>
cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- check <package-dir>
cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- simulate <package-dir> [options]
```

Run `-- --help` for the full option list (`--grant NAME=PATH`, `--request`,
`--profile-id`, and so on).

## Why a workspace member, not a script

The alternative structures considered were a standalone script (can't reuse
`plugin_manifest`/`plugin_runtime` as compiled Rust without re-parsing or
shelling out) and a `src-tauri` example (would tie the SDK to the app crate's
own feature flags and pull `tauri`/`wasmtime`-adjacent build cost into every
`cargo check` of the app). A separate workspace member costs one more crate in
the dependency graph — it does not touch `orivo`'s own binary, its Tauri
bundle, or `cargo test --manifest-path src-tauri/Cargo.toml`, which only
builds the package it's pointed at. `orivo_lib`'s `[lib]` crate-type already
includes `rlib`, so no new build target was needed on the app's side beyond
making the two modules this SDK reuses `pub` (see `src-tauri/src/lib.rs`).

## Tests

```sh
cargo test --manifest-path sdk/orivo-plugin-sdk/Cargo.toml
```

`tests/simulate_against_the_real_fixture.rs` packages the committed
`src-tauri/fixtures/orivo-runner-fixture.wasm` and runs it through this SDK's
`simulate`/`check` against the real host — the same component
`plugin_runtime.rs`'s own adversarial suite uses, not a stand-in.
`tests/wit_compatibility.rs` freezes `orivo-plugin@1` against
`tests/wit-v1-baseline.json` and fails on a breaking change to the published
contract; see that file's module doc for what counts as breaking and why.

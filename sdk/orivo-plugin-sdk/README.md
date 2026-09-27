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
# or, from the repository root:
cargo test -p orivo-plugin-sdk
```

CI runs this on Linux only, as one extra step in the `rust` job
(`.github/workflows/ci.yml`), right after `cargo check --manifest-path
src-tauri/Cargo.toml`: the crate has no platform-specific code, so one
platform is enough, and this is what actually exercises
`wit_compatibility.rs` — a compatibility guard nobody runs is not a
guard.

`tests/simulate_against_the_real_fixture.rs` packages the committed
`src-tauri/fixtures/orivo-runner-fixture.wasm` and runs it through this SDK's
`simulate`/`check` against the real host — the same component
`plugin_runtime.rs`'s own adversarial suite uses, not a stand-in.

`tests/wit_compatibility.rs` freezes `orivo-plugin@1` against
`tests/wit-v1-baseline.json` and fails on a breaking change to the published
contract; see that file's module doc for what counts as breaking and why. The
one `#[ignore]`d test in that file, `dump_current_snapshot_for_rebaselining`,
is not part of the suite: it prints the current contract's snapshot so it can
be pasted into `wit-v1-baseline.json`, and it exists for exactly one occasion
— `wit/orivo-plugin.wit` gaining a deliberate, reviewed v2. Run it by hand
with
`cargo test -p orivo-plugin-sdk --test wit_compatibility -- --ignored --nocapture dump_current_snapshot`;
never to make the frozen-baseline test above pass.

## The cost of reuse

This crate depends on `orivo` (`src-tauri`) as a path dependency, which is the
whole point — `plugin_manifest`/`plugin_runtime` cannot drift from a second
copy that does not exist — but it means building or testing this crate builds
the *entire* app crate first: Tauri, Wasmtime, the platform-specific
dependencies in `src-tauri/Cargo.toml`, and `dist/` has to exist before that
compiles at all (`tauri::generate_context!()` embeds it; run `pnpm exec vite
build` first, same as any other `cargo` invocation against this repository).
A change to this SDK alone still costs a full app build, and pulling in
`orivo` also pulls in a Node/pnpm step this crate's own logic never touches.
The alternative — copying `plugin_manifest`/`plugin_runtime` instead of
depending on them — trades that build cost for exactly the drift this SDK
exists to prevent, which is the worse trade. See **Out of scope /
follow-ups** in the PR for the real fix (a small shared crate), which is
follow-up work, not something to improvise here.

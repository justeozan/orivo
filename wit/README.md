# Orivo Plugin SDK v1

`orivo-plugin.wit` is the versioned contract between the Rust host and a
WebAssembly component. It is intentionally smaller than Orivo's product
ambition: every new permission or UI surface requires an ABI revision rather
than being hidden in JSON or a command string.

## Rules

- Components use one compatible world: `source-plugin`, `runner-plugin`,
  `metadata-plugin` or `ui-plugin`. Only `runner-plugin` is invocable today; the
  other three are still contract-only.
- `runner.prepare-launch` returns opaque IDs only. The Orivo host validates a
  profile and constructs the native process without a shell.
- Wine-Staging is the first-party native reference adapter for this runner
  contract. It does not pretend to be a bundled Wasm plugin: its Rust host
  creates the equivalent typed launch intent from catalog-owned opaque IDs,
  then applies the same no-path/no-shell boundary. Android's Winlator runner is
  the second adapter built that way, and hands its typed intent to another
  application instead of a process — see
  [`docs/winlator-runner.md`](../docs/winlator-runner.md).
- `discover-page` is cursor-based so a large library can be imported in bounded
  jobs and resumed after cancellation.
- UI contributions are data. Plugins cannot inject HTML/CSS/JavaScript or gain
  access to the WebView/Tauri IPC bridge.
- `host-journal` and `host-files` are the only imports the host provides. There
  is no WASI: a component that imports a clock, a socket, a random source or a
  preopened directory has nothing to link against and is refused before it is
  instantiated. Adding an import to a world is backward-compatible — a component
  built against the older world simply does not use it — which is why these
  arrived inside v1 rather than as v2.
- `host-files` is scoped by grant, never by path. A plugin names an opaque
  directory grant the user approved and a single ordinary path component inside
  it; only the host knows which folder that is. It rejects separators, `.`, `..`,
  control characters and `:` (a Windows drive prefix discards the grant
  entirely), opens the entry without following a link and without blocking, and
  then refuses anything that is not a regular file of an allowed size.
- Grants decide two different things. The *manifest* decides what is linked: an
  import the package never declared is absent, so a component needing it cannot
  be instantiated at all. The *grant* decides what works: a declared but
  ungranted capability is linked and refuses every call with a typed
  `plugin-error`. That is what lets Orivo run `get-identity` and `health-check`,
  under the host's limits and with no authority, before asking the user to grant
  anything.
- Every call is bounded by the host: fuel, an epoch deadline, memory per instance
  and across instances. A component cannot observe or raise them, and a call that
  does not return is interrupted.
- `src-tauri/fixtures/runner-fixture` is the reference third-party component for
  this contract, and the host's tests are written against it.

The host validates package identity and capabilities in
`src-tauri/src/plugin_manifest.rs`, invokes components in
`src-tauri/src/plugin_runtime.rs` and queues those invocations in
`src-tauri/src/plugin_scheduler.rs`; the WIT file must remain aligned with its
`orivo-plugin@1` SDK identifier.

## Building a runner plugin from zero

`sdk/orivo-plugin-sdk` is the developer kit: a manifest validator and a host
simulator that call the same code Orivo runs (`plugin_manifest`,
`plugin_runtime`), so nothing here can silently validate against a second,
drifted copy of the real rules. Its own README covers each subcommand's
options; this is the shortest path from an empty directory to a runner that
answers a simulated call.

1. **Start from the template.** Copy
   `sdk/orivo-plugin-sdk/templates/runner-plugin-starter` and rename its
   `PLUGIN_ID`, `PLUGIN_VERSION` and `LIBRARY_GRANT` constants. It implements
   `runner-plugin` honestly — one directory grant, no misbehaviour — unlike
   `src-tauri/fixtures/runner-fixture`, which is deliberately adversarial and
   exists to test the host, not to be copied by an author.
2. **Build the component.**
   ```sh
   rustup target add wasm32-unknown-unknown
   cargo install wasm-tools --locked --version 1.246.2
   ./build.sh   # prints component.wasm's sha256 and byte size
   ```
3. **Write the manifest.** Copy `manifest.json.example` to `manifest.json` and
   fill in the `sha256`/`byteSize` `build.sh` printed. `id` must be a
   lowercase reverse-DNS string with at least three labels; `sdk` must stay
   `orivo-plugin@1`.
4. **Validate it.**
   ```sh
   cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- validate <your-plugin-dir>
   ```
   This runs `PluginManifest::validate` and, once a `signature.ed25519` file
   exists (any bytes — the SDK only speaks for the development channel),
   `validate_plugin_package` — the exact checks the installer applies, plus a
   direct byte comparison against the declared artifact hash.
5. **Check the contract, then simulate a call.**
   ```sh
   cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- check <your-plugin-dir>
   cargo run --manifest-path sdk/orivo-plugin-sdk/Cargo.toml -- simulate <your-plugin-dir> \
     --request discover-page --grant library=./fixtures/games
   ```
   `check` reproduces the registry's pre-install gate: the component's own
   type must ask for no more than the manifest declares, and `get-identity`
   must agree with it. `simulate` runs one call through the real
   `PluginRuntime` — the same fuel, epoch deadline and memory ceilings Orivo
   enforces — and prints the host's journal alongside the answer, so a
   `permission-denied` shows *why* rather than only that one happened.
6. **Package it.** `.orivo-plugin` archives, signing and the registry are the
   installer's territory (`src-tauri/src/plugin_installer.rs`,
   `src-tauri/src/plugin_registry.rs`), not this SDK's. Until that flow is
   available to third parties, install a validated directory through Orivo's
   development channel exactly as the installer's own tests do.

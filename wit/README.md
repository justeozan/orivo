# Orivo Plugin SDK v1

`orivo-plugin.wit` is the versioned contract between the Rust host and a
WebAssembly component. It is intentionally smaller than Orivo's product
ambition: every new permission or UI surface requires an ABI revision rather
than being hidden in JSON or a command string.

## Rules

- Components use one compatible world: `source-plugin`, `runner-plugin`,
  `metadata-plugin` or `ui-plugin`.
- `runner.prepare-launch` returns opaque IDs only. The Orivo host validates a
  profile and constructs the native process without a shell.
- Wine-Staging is the first-party native reference adapter for this runner
  contract. It does not pretend to be a bundled Wasm plugin: its Rust host
  creates the equivalent typed launch intent from catalog-owned opaque IDs,
  then applies the same no-path/no-shell boundary. Third-party components
  remain preflight-only until WIT invocation and grants are implemented.
- `discover-page` is cursor-based so a large library can be imported in bounded
  jobs and resumed after cancellation.
- UI contributions are data. Plugins cannot inject HTML/CSS/JavaScript or gain
  access to the WebView/Tauri IPC bridge.
- Host grants are not part of this contract yet. They are resolved by the host
  before it invokes a capability-specific function.

The host validates package identity and capabilities in
`src-tauri/src/plugin_manifest.rs`; the WIT file must remain aligned with its
`orivo-plugin@1` SDK identifier.

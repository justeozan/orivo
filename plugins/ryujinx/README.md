# Ryujinx runner plugin

Orivo's first official runner shipped as a WebAssembly component. It lists one
granted folder, recognises the six file types Ryujinx's own library scanner
opens, and prepares a typed launch intent. It never reads a file, so the keys,
firmware and saves that live in the same folder are out of reach by
construction.

What it does and why, what Ryujinx accepts and where that was read from, what
the v1 contract cannot express, and how to publish it: **[`docs/ryujinx-runner.md`](../../docs/ryujinx-runner.md)**.

```text
src/lib.rs                the component
build.sh                  builds it; prints the sha256 and byte size to paste
package/manifest.json     the package Orivo ships
package/component.wasm    the committed artefact; `cargo test` checks its digest
```

Tests: `src-tauri/src/ryujinx_plugin.rs` (the real host path — install, profile,
grant, import, process) and `sdk/orivo-plugin-sdk/tests/ryujinx_plugin.rs` (the
published validator and simulator, on the package we publish).

Orivo never ships Ryujinx, a key, a firmware dump or a game.

#!/bin/sh
# Rebuild the committed Ryujinx runner component and print the two values
# `package/manifest.json` has to agree with.
#
# `cargo test` reads `plugins/ryujinx/package/component.wasm` and checks its
# SHA-256; it never runs this script. That is the point, and it is the same rule
# `src-tauri/fixtures/runner-fixture/build.sh` follows: a WebAssembly target and
# a component tool are needed to *change* this plugin, not to run Orivo's test
# suite on a fresh clone.
#
# Requires, and checks for:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-tools --locked --version 1.246.2
#
# wasm-tools is pinned to the release whose component encoding matches the
# wasmtime the host depends on — see `src-tauri/fixtures/README.md`.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/package/component.wasm"
MODULE="$HERE/target/wasm32-unknown-unknown/release/orivo_ryujinx_runner.wasm"

command -v wasm-tools >/dev/null 2>&1 || {
  echo "build.sh: wasm-tools is not on PATH (cargo install wasm-tools --locked --version 1.246.2)" >&2
  exit 2
}
rustup target list --installed | grep -qx wasm32-unknown-unknown || {
  echo "build.sh: rustup target add wasm32-unknown-unknown" >&2
  exit 2
}

cd "$HERE"
cargo build --release --target wasm32-unknown-unknown --locked
wasm-tools component new "$MODULE" -o "$OUT"
# Producers and component-type sections are build provenance, not contract.
# Dropping them keeps the committed artefact small and byte-stable across
# toolchain patch releases, which is what makes a committed digest reproducible.
wasm-tools strip -d 'producers' -d 'component-type.*' "$OUT" -o "$OUT.stripped"
mv "$OUT.stripped" "$OUT"
wasm-tools validate --features component-model "$OUT"

printf '%s  %s  %s bytes\n' \
  "$(shasum -a 256 "$OUT" | cut -d' ' -f1)" \
  "component.wasm" \
  "$(wc -c <"$OUT" | tr -d ' ')"
echo
echo 'Paste both into package/manifest.json (artifacts[0].sha256 / byteSize) and'
echo 'nothing else: src-tauri/src/ryujinx_plugin.rs reads them out of that file, so'
echo 'a stale digest fails its own test before anything else does.'

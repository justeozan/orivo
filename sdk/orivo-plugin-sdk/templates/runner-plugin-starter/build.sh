#!/bin/sh
# Builds this starter into a component and prints its SHA-256, the value
# `manifest.json`'s `artifacts[0].sha256` and `byteSize` must match.
#
# Requires, and checks for, the same toolchain as
# `src-tauri/fixtures/runner-fixture/build.sh`:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-tools --locked --version 1.246.2
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/component.wasm"
MODULE="$HERE/target/wasm32-unknown-unknown/release/orivo_runner_plugin_starter.wasm"

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
wasm-tools strip -d 'producers' -d 'component-type.*' "$OUT" -o "$OUT.stripped"
mv "$OUT.stripped" "$OUT"
wasm-tools validate --features component-model "$OUT"

printf '%s  %s  %s bytes\n' \
  "$(shasum -a 256 "$OUT" | cut -d' ' -f1)" \
  "$(basename "$OUT")" \
  "$(wc -c <"$OUT" | tr -d ' ')"

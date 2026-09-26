#!/bin/sh
# Rebuild the committed runner fixture component.
#
# `cargo test` reads `src-tauri/fixtures/orivo-runner-fixture.wasm` and checks
# its SHA-256; it never runs this script. That is the point: a WebAssembly
# target, a component tool and a network fetch are needed to *change* the
# fixture, not to run Orivo's test suite on a fresh clone.
#
# Requires, and checks for:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-tools --locked --version 1.246.2
#
# wasm-tools is pinned to the release whose component encoding matches the
# wasmtime the host depends on (44.0.0 → wasm-encoder 0.246.2).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
FIXTURES="$HERE/.."
OUT="$FIXTURES/orivo-runner-fixture.wasm"
MODULE="$HERE/target/wasm32-unknown-unknown/release/orivo_runner_fixture.wasm"

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
# The producers and component-type sections are build provenance, not contract.
# Dropping them keeps the committed artefact small and byte-stable across
# toolchain patch releases.
wasm-tools strip -d 'producers' -d 'component-type.*' "$OUT" -o "$OUT.stripped"
mv "$OUT.stripped" "$OUT"
wasm-tools validate --features component-model "$OUT"

# The refusal fixtures are hand-written component text; they need no guest
# toolchain at all, only the same encoder.
wasm-tools parse "$FIXTURES/wasi-import.wat" -o "$FIXTURES/wasi-import.wasm"
wasm-tools parse "$FIXTURES/memory64.wat" -o "$FIXTURES/memory64.wasm"

for artefact in "$OUT" "$FIXTURES/wasi-import.wasm" "$FIXTURES/memory64.wasm"; do
  printf '%s  %s  %s bytes\n' \
    "$(shasum -a 256 "$artefact" | cut -d' ' -f1)" \
    "$(basename "$artefact")" \
    "$(wc -c <"$artefact" | tr -d ' ')"
done

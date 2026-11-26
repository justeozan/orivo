#!/bin/sh
# Rebuild the committed Store tab fixture component.
#
# `cargo test` reads `src-tauri/fixtures/orivo-store-tab-fixture.wasm` and
# checks its SHA-256; it never runs this script. Same toolchain and pins as
# `../runner-fixture/build.sh`, kept separate so changing this fixture never
# rebuilds the runner one under it.
#
# Requires, and checks for:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-tools --locked --version 1.246.2
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
FIXTURES="$HERE/.."
OUT="$FIXTURES/orivo-store-tab-fixture.wasm"
MODULE="$HERE/target/wasm32-unknown-unknown/release/orivo_store_tab_fixture.wasm"

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
# Build provenance, not contract: dropped so the artefact stays byte-stable.
wasm-tools strip -d 'producers' -d 'component-type.*' "$OUT" -o "$OUT.stripped"
mv "$OUT.stripped" "$OUT"
wasm-tools validate --features component-model "$OUT"

printf '%s  %s  %s bytes\n' \
  "$(shasum -a 256 "$OUT" | cut -d' ' -f1)" \
  "$(basename "$OUT")" \
  "$(wc -c <"$OUT" | tr -d ' ')"

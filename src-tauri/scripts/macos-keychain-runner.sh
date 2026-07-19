#!/bin/sh
set -eu

app="$1"
shift

# Keep a deterministic designated requirement for local `cargo run` / `tauri
# dev` sessions. Distribution builds must still use the project's real Apple
# signing identity, but this prevents a debug rebuild from losing access to a
# Keychain item created one restart earlier.
/usr/bin/codesign --force --sign - --identifier io.orivo.desktop "$app"
exec "$app" "$@"

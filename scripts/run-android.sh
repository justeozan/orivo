#!/usr/bin/env bash
# The one Android command: launch Orivo on a real phone when one is reachable,
# and fall back to the emulator when none is. macOS-only, meant for local
# Conductor workspaces.
#
#   ./scripts/run-android.sh              # phone if paired, emulator otherwise
#   ./scripts/run-android.sh --emulator   # force the AVD
#   ./scripts/run-android.sh --device     # force hardware (fails if absent)
#
# Everything else is passed straight through to the script that ends up
# running, so --apk/--serial/--pair/--avd all still work.
#
# Hardware wins by default because it is the only host that answers the
# question docs/performance.md §6 says an emulator cannot — real GPU, real
# thermals. A phone counts as reachable if adb already lists it (USB or an
# earlier wireless pairing) or if wireless debugging is advertising it over
# mDNS, in which case run-android-device.sh asks for the pairing code itself.
set -euo pipefail

cd "$(dirname "$0")/.."

MODE="auto"
case "${1:-}" in
  --emulator) MODE="emulator"; shift ;;
  --device) MODE="device"; shift ;;
esac

# The SDK ships adb only inside its own tree.
if ! command -v adb >/dev/null 2>&1; then
  SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  [ -d "$SDK_ROOT/platform-tools" ] && export PATH="$SDK_ROOT/platform-tools:$PATH"
fi

has_physical_device() {
  command -v adb >/dev/null 2>&1 || return 1
  adb devices 2>/dev/null \
    | awk '$2 == "device" && $1 !~ /^emulator-/ && $1 !~ /_adb-/ { found = 1 } END { exit !found }'
}

has_pairable_device() {
  command -v adb >/dev/null 2>&1 || return 1
  adb mdns services 2>/dev/null | grep -q '_adb-tls-connect'
}

if [ "$MODE" = "auto" ]; then
  if has_physical_device; then
    echo "android: a phone is attached — using it."
    MODE="device"
  elif has_pairable_device; then
    echo "android: a phone is advertising wireless debugging — pairing with it."
    MODE="device"
  else
    echo "android: no phone reachable — falling back to the emulator."
    MODE="emulator"
  fi
fi

case "$MODE" in
  device) exec ./scripts/run-android-device.sh "$@" ;;
  emulator) exec ./scripts/run-android-emulator.sh "$@" ;;
esac

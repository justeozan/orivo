#!/usr/bin/env bash
# Boot the Android emulator, wait for it to finish booting, then launch Orivo
# on it. macOS-only, meant for local Conductor workspaces.
#
# One-time prerequisites:
#   Android Studio (provides `emulator` and `adb` on PATH)
#   pnpm tauri android init           (the Android project lives in the
#                                      gitignored src-tauri/gen/, so a fresh
#                                      clone has to generate it once)
#
# `tauri android dev` builds, installs and launches in one go, so the build is
# only kicked off when the app is not on the device yet. Pass an APK to skip it:
#   ./scripts/run-android-emulator.sh --apk path/to/app-universal-debug.apk
#
# Only one emulator may run at a time — docs/agent-workplan.md calls this the
# emulator lock — so the script reuses a device that is already up instead of
# starting a second one.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

AVD="${ORIVO_AVD:-Orivo_Test}"
PKG="io.orivo.desktop"
ACTIVITY="${PKG}/.MainActivity"
BOOT_TIMEOUT="${ORIVO_BOOT_TIMEOUT:-180}"
APK=""

while [ $# -gt 0 ]; do
  case "$1" in
    --apk) APK="${2:-}"; shift 2 ;;
    --avd) AVD="$2"; shift 2 ;;
    *) echo "usage: $0 [--apk <apk>] [--avd <name>]" >&2; exit 2 ;;
  esac
done

# 1. Toolchain: the Android SDK ships both tools but only inside its own tree.
if ! command -v adb >/dev/null 2>&1 || ! command -v emulator >/dev/null 2>&1; then
  SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  if [ -d "$SDK_ROOT/platform-tools" ]; then
    export PATH="$SDK_ROOT/platform-tools:$SDK_ROOT/emulator:$PATH"
  fi
fi
for tool in adb emulator; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool: not found. Install Android Studio, or set ANDROID_SDK_ROOT." >&2
    exit 1
  fi
done

emulator_serial() {
  adb devices | awk '$2 == "device" && $1 ~ /^emulator-/ { print $1; exit }'
}

# 2. Reuse a running emulator, otherwise boot the AVD detached from this shell
#    so closing the terminal (or the Conductor run) does not kill it.
SERIAL="$(emulator_serial)"
if [ -z "$SERIAL" ]; then
  if ! emulator -list-avds | grep -qx "$AVD"; then
    echo "emulator: no AVD named '${AVD}'. Available:" >&2
    emulator -list-avds | sed 's/^/  /' >&2
    exit 1
  fi
  echo "emulator: booting ${AVD}..."
  nohup emulator -avd "$AVD" -no-boot-anim >/dev/null 2>&1 &
  adb wait-for-device
  SERIAL="$(emulator_serial)"
  if [ -z "$SERIAL" ]; then
    echo "emulator: ${AVD} did not register with adb." >&2
    exit 1
  fi
fi

# 3. `adb wait-for-device` returns as soon as adbd answers, which is long before
#    the framework is up — package manager and am start both fail until
#    sys.boot_completed flips to 1.
echo "emulator: ${SERIAL} is up, waiting for the framework to finish booting..."
for _ in $(seq 1 "$BOOT_TIMEOUT"); do
  if [ "$(adb -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; then
    break
  fi
  sleep 1
done
if [ "$(adb -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" != "1" ]; then
  echo "emulator: ${SERIAL} did not boot within ${BOOT_TIMEOUT}s." >&2
  exit 1
fi
adb -s "$SERIAL" shell input keyevent 82 >/dev/null 2>&1 || true
echo "emulator: ${SERIAL} booted (API $(adb -s "$SERIAL" shell getprop ro.build.version.sdk | tr -d '\r'))."

# 4. Make sure the app is on the device.
installed() {
  adb -s "$SERIAL" shell pm path "$PKG" 2>/dev/null | grep -q '^package:'
}

if [ -n "$APK" ]; then
  [ -f "$APK" ] || { echo "apk: no such file: ${APK}" >&2; exit 1; }
  echo "apk: installing ${APK}..."
  adb -s "$SERIAL" install -r -g "$APK"
elif ! installed; then
  if [ ! -d "${ROOT}/src-tauri/gen/android" ]; then
    echo "android: ${ROOT}/src-tauri/gen/android is missing (it is gitignored)." >&2
    echo "  Run this once: pnpm tauri android init" >&2
    exit 1
  fi
  echo "android: ${PKG} is not installed — building and installing it..."
  echo "  (Ctrl-C stops the build but leaves the emulator running)"
  exec pnpm tauri android dev --no-watch
fi

# 5. Launch. A force-stop first, so the measurement in docs/performance.md
#    (am start -W after force-stop) and the run here are the same thing.
echo "android: launching Orivo..."
adb -s "$SERIAL" shell am force-stop "$PKG"
exec adb -s "$SERIAL" shell am start -W -n "$ACTIVITY"
#!/usr/bin/env bash
# Launch Orivo on a physical Android device over ADB — USB or wireless
# debugging. macOS-only, meant for local Conductor workspaces.
#
# Unlike scripts/run-android-emulator.sh this never boots an emulator: the
# point is to measure on real silicon, since docs/performance.md §6 records
# emulator numbers that are explicitly flagged as unrepresentative.
#
#   ./scripts/run-android-device.sh                      # the only command
#   ./scripts/run-android-device.sh --serial 192.168.1.28:37441
#   ./scripts/run-android-device.sh --pair 022531       # skip the code prompt
#   ./scripts/run-android-device.sh --apk path/to.apk
#
# One command covers every case: a phone already paired connects silently, and
# one that is on the network but unpaired is detected, and the script asks for
# the 6-digit code itself before pairing. Nothing else is needed at the prompt.
#
# Wireless debugging (HyperOS: Réglages > Options pour les développeurs >
# Débogage sans fil):
#
#   * Already paired once? `adb connect <ip>:<port>` is enough, forever. The
#     pairing only has to be redone if the phone's network config is reset.
#   * First time, or after a reset: open "Appairer un appareil avec un code
#     d'appairage", then pass the 6-digit code to --pair. The pairing code is
#     single-use and expires in minutes, so --pair re-discovers the pairing
#     port immediately before dialling — the phone reassigns it every time the
#     dialog is opened, and a stale port fails with a bare
#     "protocol fault (couldn't read status message)".
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

PKG="io.orivo.desktop"
ACTIVITY="${PKG}/.MainActivity"
SERIAL=""
PAIR_CODE=""
APK=""
RETRIES="${ORIVO_CONNECT_RETRIES:-3}"

while [ $# -gt 0 ]; do
  case "$1" in
    --serial) SERIAL="${2:-}"; shift 2 ;;
    --pair) PAIR_CODE="${2:-}"; shift 2 ;;
    --apk) APK="${2:-}"; shift 2 ;;
    *) echo "usage: $0 [--serial <serial>] [--pair <code>] [--apk <apk>]" >&2; exit 2 ;;
  esac
done

# 1. Toolchain. The SDK ships both tools but only inside its own tree.
if ! command -v adb >/dev/null 2>&1; then
  SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
  [ -d "$SDK_ROOT/platform-tools" ] && export PATH="$SDK_ROOT/platform-tools:$PATH"
fi
command -v adb >/dev/null 2>&1 || { echo "adb: not found." >&2; exit 1; }

# 2. Pick a device. Physical first: an emulator on the same bus would
#    otherwise win by accident, and the whole point is to measure on hardware.
physical_devices() {
  adb devices | awk '$2 == "device" && $1 !~ /^emulator-/ && $1 !~ /_adb-/ { print $1 }'
}

tmp="$(mktemp)"; trap 'rm -f "$tmp"' EXIT

lookup() {
  : > "$tmp"
  dns-sd -L "$1" "$2._tcp" local. > "$tmp" 2>/dev/null &
  local pid=$!
  sleep "${3:-4}"
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  sed -n 's/.*can be reached at [^ ]*:\([0-9][0-9]*\).*/\1/p' "$tmp" | head -1
}

# Any adb mdns service instance name works; the connect one is the stable
# handle because it survives the pairing dialog closing.
mdns_service() {
  adb mdns services 2>/dev/null | awk '/_adb-tls-connect/ { print $1; exit }'
}

if [ -n "$PAIR_CODE" ] || { [ -z "$SERIAL" ] && [ -z "$(physical_devices)" ] && [ -n "$(mdns_service)" ]; }; then
  # 3. Pair over wireless debugging. Both service names are advertised over
  #    mDNS; the pairing one is the only one that accepts `adb pair`.
  service="$(mdns_service)"
  [ -n "$service" ] || { echo "pair: no ADB device on the network — is Wireless debugging on?" >&2; exit 1; }

  addr="$(lookup "$service" _adb-tls-connect 4)"
  [ -n "$addr" ] || { echo "pair: could not resolve ${service}." >&2; exit 1; }
  host="${addr%%:*}"

  # No --pair given and nothing connected: this is the first run, so the code
  # does not exist yet. Ask for it rather than making the caller pass a flag —
  # the point of this script is that one command covers every case.
  if [ -z "$PAIR_CODE" ]; then
    echo "pair: ${host} is on the network but not paired with this Mac."
    echo "pair: on the phone, open"
    echo "        Réglages > Options pour les développeurs > Débogage sans fil"
    echo "        > Appairer un appareil avec un code d'appairage"
    printf 'pair: 6-digit code (empty to abort): '
    read -r PAIR_CODE
    [ -n "$PAIR_CODE" ] || { echo "pair: aborted." >&2; exit 1; }
  fi

  paired=0
  for attempt in $(seq 1 "$RETRIES"); do
    port="$(lookup "$service" _adb-tls-pairing 4)"
    if [ -z "$port" ]; then
      echo "pair [${attempt}]: pairing service not advertised — open the pairing dialog again."
      sleep 2
      continue
    fi
    echo "pair [${attempt}]: ${host}:${port}"
    if out="$(printf '%s\n' "$PAIR_CODE" | adb pair "${host}:${port}" 2>&1)" \
       && printf '%s' "$out" | grep -q "Successfully paired"; then
      echo "pair: ${out}"
      paired=1
      break
    fi
    echo "pair [${attempt}]: ${out}"
    # A stale code fails on the first port the phone used; the next attempt
    # re-reads mDNS, because the phone restarts the service and moves the port.
    sleep 1
  done
  [ "$paired" = 1 ] || { echo "pair: failed — the code is single-use, take a fresh one." >&2; exit 1; }

  SERIAL="${host}:$(lookup "$service" _adb-tls-connect 4)"
  adb connect "$SERIAL"
  sleep 2
fi

if [ -z "$SERIAL" ]; then
  # Not `mapfile` — macOS still ships bash 3.2, where it does not exist.
  devices=""
  while IFS= read -r line; do
    [ -n "$line" ] && devices="${devices}${line}
"
  done < <(physical_devices)
  count="$(printf '%s' "$devices" | grep -c . || true)"
  if [ "$count" -eq 0 ]; then
    echo "device: no physical Android device on adb." >&2
    echo "  USB: plug it in and accept 'Allow USB debugging' on the phone." >&2
    echo "  Wi-Fi: enable Wireless debugging, then rerun with --pair <code>." >&2
    echo "  See: adb devices -l" >&2
    exit 1
  fi
  SERIAL="$(printf '%s' "$devices" | head -1)"
  [ "$count" -gt 1 ] && echo "device: several phones attached, using ${SERIAL} (--serial to pick another)."
fi

# A TCP serial drops whenever the phone changes network, so one blind `adb
# connect` is not enough — retry, and fail loudly rather than installing to
# nothing.
if ! adb -s "$SERIAL" get-state 2>/dev/null | grep -q "^device$"; then
  case "$SERIAL" in
    *:*)
      for attempt in $(seq 1 "$RETRIES"); do
        echo "connect [${attempt}]: ${SERIAL}"
        adb connect "$SERIAL" >/dev/null 2>&1 || true
        sleep 2
        adb -s "$SERIAL" get-state 2>/dev/null | grep -q "^device$" && break
        sleep 1
      done
      ;;
  esac
fi
if ! adb -s "$SERIAL" get-state 2>/dev/null | grep -q "^device$"; then
  echo "device: ${SERIAL} is not ready." >&2
  exit 1
fi

model="$(adb -s "$SERIAL" shell getprop ro.product.model 2>/dev/null | tr -d '\r')"
sdk="$(adb -s "$SERIAL" shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r')"
abi="$(adb -s "$SERIAL" shell getprop ro.product.cpu.abi 2>/dev/null | tr -d '\r')"
echo "device: ${model} (${SERIAL}, API ${sdk}, ${abi})"

installed() {
  adb -s "$SERIAL" shell pm path "$PKG" 2>/dev/null | grep -q '^package:'
}

# 4. Get the app onto the phone.
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
  echo "  (Ctrl-C stops the build but leaves the phone untouched)"
  exec pnpm tauri android dev --no-watch
fi

# 5. Launch. force-stop first so this is the same measurement
#    docs/performance.md §6 records (am start -W after force-stop).
echo "android: launching Orivo on ${model}..."
adb -s "$SERIAL" shell am force-stop "$PKG"
exec adb -s "$SERIAL" shell am start -W -n "$ACTIVITY"
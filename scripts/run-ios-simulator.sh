#!/usr/bin/env bash
# Launch Orivo in an iOS simulator through `pnpm tauri ios dev`. macOS-only,
# meant for local Conductor workspaces.
#
#   ./scripts/run-ios-simulator.sh                 # pick a simulator
#   ./scripts/run-ios-simulator.sh --device "iPhone 17 Pro"
#
# One-time prerequisites:
#   Xcode (with the iOS Simulator platform installed)
#   pnpm tauri ios init      (the project lives in the gitignored
#                             src-tauri/gen/, so a fresh clone generates it)
#
# Why this exists rather than calling `pnpm tauri ios dev` directly:
#
#   Tauri only prompts for a device when it is given no [DEVICE] argument. That
#   prompt lives in cargo-mobile2's `prompt::list`, which loops forever on an
#   empty line — and a non-TTY stdin returns EOF, so every read is empty. Run
#   it from a script and it does not sit waiting for input, it prints
#   "Not to be pushy, but you need to pick a device." until the disk is full
#   (measured: 15 million lines, 850 MB, in ~40 s). Passing the device name
#   skips the prompt outright, and a name that matches nothing bails with a
#   plain error instead of looping.
#
#   Ten of the installed simulator names are duplicated across runtimes (iOS
#   26.5 and 27.0 both ship "iPhone 17"), so a name alone can be ambiguous for
#   Tauri's fuzzy matcher. The pick below therefore prefers names that resolve
#   to exactly one simulator.
#
#   The name is matched against physical devices first: `tauri ios dev` asks
#   cargo-mobile2 for connected hardware and only falls back to the simulator
#   list when that comes up empty. Since Xcode 26, `xcrun devicectl list
#   devices` reports simulators next to real phones (`Reality = simulated`,
#   `visibilityClass = simulators`), and cargo-mobile2 only learned to skip them
#   in 0.22.5 — which first shipped in @tauri-apps/cli 2.12.0. On 2.11.x every
#   installed simulator looks like a connected iPhone, the fuzzy matcher picks
#   the closest name, and the build then runs `xcodebuild -sdk iphoneos
#   -allowProvisioningUpdates` and dies on "No code signing certificates
#   found". package.json pins the CLI for that reason, and the check below
#   refuses to start on an older one rather than letting it fail confusingly
#   minutes later.
#
#   With a CLI that does skip them, booting the simulator ourselves is safe:
#   simulators are filtered out of the device list whether they are booted or
#   shut down.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
PBXPROJ="${ROOT}/src-tauri/gen/apple/orivo.xcodeproj/project.pbxproj"
PROJECT_YML="${ROOT}/src-tauri/gen/apple/project.yml"

# Xcode 27's simulator SDK accepts IPHONEOS_DEPLOYMENT_TARGET in 15.0–27.0.x
# only. Tauri's default is 14.0, baked into the project by xcodegen at
# `tauri ios init` time, which fails the build with
#   error: The iOS Simulator deployment target 'IPHONEOS_DEPLOYMENT_TARGET' is
#         set to 14.0, but the range of supported ... is 15.0 to 27.0.x
# src-tauri/tauri.conf.json now sets bundle.iOS.minimumSystemVersion = 15.0, so
# a fresh init is already correct; this heals an init generated before that.
MIN_IOS="15.0"

DEVICE="${ORIVO_IOS_DEVICE:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="${2:-}"; shift 2 ;;
    *) echo "usage: $0 [--device <simulator name>]" >&2; exit 2 ;;
  esac
done

command -v xcrun >/dev/null 2>&1 || { echo "xcrun: not found — install Xcode." >&2; exit 1; }
command -v pnpm >/dev/null 2>&1 || { echo "pnpm: not found." >&2; exit 1; }

# See the header: an older CLI runs the app on "hardware" that is really a
# simulator, and fails on code signing.
MIN_CLI="2.12.0"
cli_version="$(pnpm tauri --version 2>/dev/null | awk '$1 == "tauri-cli" { print $2 }')"
if [ -n "$cli_version" ] &&
  [ "$(printf '%s\n%s\n' "$MIN_CLI" "$cli_version" | sort -V | head -1)" != "$MIN_CLI" ]; then
  echo "ios: @tauri-apps/cli ${cli_version} cannot target the simulator (needs ${MIN_CLI})." >&2
  echo "  Run: pnpm install" >&2
  exit 1
fi

# `beforeDevCommand` is `pnpm dev`, and vite is configured with strictPort, so a
# dev server another session left behind makes Tauri exit on "Port 5173 is
# already in use" with the frontend error buried above its own.
if stale_dev="$(lsof -ti tcp:5173 2>/dev/null)" && [ -n "$stale_dev" ]; then
  echo "ios: something already listens on 127.0.0.1:5173 (pid ${stale_dev//$'\n'/, })." >&2
  echo "  \`pnpm dev\` cannot bind it. Stop it first: kill ${stale_dev//$'\n'/ }" >&2
  exit 1
fi

if [ ! -d "${ROOT}/src-tauri/gen/apple" ]; then
  echo "ios: src-tauri/gen/apple is missing (it is gitignored)."
  echo "  Generating it: pnpm tauri ios init --ci"
  pnpm tauri ios init --ci
fi

# Self-heal the deployment target, in both places it lives.
if [ -f "$PROJECT_YML" ]; then
  perl -pi -e "s/^(    iOS: )14\\.0\$/\$1${MIN_IOS}/" "$PROJECT_YML"
fi
if [ -f "$PBXPROJ" ]; then
  perl -pi -e "s/(\bIPHONEOS_DEPLOYMENT_TARGET = )14\\.0;/\$1${MIN_IOS};/g" "$PBXPROJ"
fi

# Pick the simulator. xcrun's JSON is the only list that separates runtimes;
# the plain-text form omits them, so "iPhone 17" cannot be told apart by name.
if [ -z "$DEVICE" ]; then
  DEVICE="$(xcrun simctl list devices --json | python3 -c '
import json, sys
from collections import Counter

all_devices = json.load(sys.stdin)["devices"]
pool = [
    d for runtime, devices in all_devices.items()
    if "iOS" in runtime for d in devices if d.get("isAvailable", True)
]
counts = Counter(d["name"] for d in pool)
unique = [d for d in pool if counts[d["name"]] == 1]
booted = [d for d in unique if d["state"] == "Booted"]
preferred = ["iPhone 18 Pro", "iPhone 17 Pro", "iPhone 18 Pro Max", "iPhone 17 Pro Max"]

if booted:
    pick = booted[0]
else:
    pick = next((d for d in pool if d["name"] in preferred), None)
    pick = pick or (unique[0] if unique else (pool[0] if pool else None))

if pick is None:
    sys.exit("no iOS simulator installed — run: xcodebuild -downloadPlatform iOS")
print(pick["name"])
')"
fi
echo "simulator: ${DEVICE}"

# Boot it ourselves so the simulator window is up and the choice is confirmed
# before Tauri spends minutes compiling. Tauri starts it too if we did not.
resolved="$(xcrun simctl list devices --json | python3 -c "
import json, sys
name = '''${DEVICE}'''
for devices in json.load(sys.stdin)['devices'].values():
    for d in devices:
        if d['name'] == name:
            print(d['udid'], d['state'])
            raise SystemExit(0)
sys.exit(1)
" 2>/dev/null)" || {
  echo "simulator: no simulator named '${DEVICE}'. Installed:" >&2
  xcrun simctl list devices | sed -n '/^-- iOS/,/^$/p' | sed 's/^/  /' >&2
  exit 1
}
udid="${resolved% *}"
state="${resolved##* }"

if [ "$state" != "Booted" ]; then
  echo "simulator: booting ${DEVICE}..."
  xcrun simctl boot "$udid" 2>/dev/null || true
  for _ in $(seq 1 60); do
    state="$(xcrun simctl list devices | grep -F "$udid" | grep -o 'Booted' || true)"
    [ -n "$state" ] && break
    sleep 1
  done
fi
if [ "$state" != "Booted" ]; then
  echo "simulator: ${DEVICE} did not boot within 60s." >&2
  exit 1
fi
echo "simulator: ${DEVICE} is up (${udid})."

# The name is mandatory: see the header. exec keeps the build and the app
# attached to this terminal so Orca's stop button reaches them.
exec pnpm tauri ios dev "$DEVICE"
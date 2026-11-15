#!/usr/bin/env bash
# Build Orivo's sideload edition for iPhone and iPad: an unsigned .ipa that
# AltStore, SideStore or Sideloadly sign with the player's own Apple ID, and
# the facts about it that the AltStore source on orivo.evey.run is built from.
#
#   scripts/build-ios-sideload.sh [out-dir]        # default: target/ios-sideload
#
# Writes, for the version in src-tauri/tauri.conf.json:
#
#   Orivo_<version>_ios-sideload.ipa    the app, unsigned
#   Orivo_<version>_ios-sideload.json   bundle id, versions, minimum iOS, size,
#                                       sha256, entitlements and privacy keys
#
# The edition is the App Store app under another identifier
# (src-tauri/tauri.ios-sideload.conf.json), so it installs next to it and is
# never mistaken for it. The facts are read back from the built app rather
# than written down here: AltStore refuses an app whose source does not list
# every entitlement and privacy permission it carries, and a list kept by hand
# is a list that drifts. See docs/ios-sideload.md.
#
# macOS with Xcode, and Rust's aarch64-apple-ios target with the llvm-tools
# component (rustup component add llvm-tools).
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="${1:-target/ios-sideload}"
CONF="src-tauri/tauri.ios-sideload.conf.json"
RELEASE="target/aarch64-apple-ios/release"
BUILT="src-tauri/gen/apple/build/arm64/Orivo.ipa"

VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
IDENTIFIER="$(node -p "require('./${CONF}').identifier")"
NAME="Orivo_${VERSION}_ios-sideload"

# `src-tauri/gen/apple` is gitignored, so a fresh checkout has none.
if [ ! -d src-tauri/gen/apple ]; then
  pnpm tauri ios init --ci
fi
scripts/sync-ios-project.sh "$CONF"

build() {
  pnpm tauri ios build --no-sign --ci --target aarch64 --config "$CONF"
}

# Xcode 27 makes every @_cdecl function of a release Swift build local.
# swift-rs promotes its archives' own functions back, but not the three of its
# runtime every archive embeds, so the first release link stops on "Undefined
# symbols" for those. Promote them in Tauri's archive alone (a second global
# copy would be a duplicate symbol) and make Cargo bundle the `tauri` crate
# again without re-running its build script. docs/app-store.md has the long
# version; an Xcode that does not internalise them never comes here.
promote_swift_rs_runtime() {
  local host objcopy archive fingerprint
  host="$(rustc -vV | sed -n 's/^host: //p')"
  objcopy="$(rustc --print sysroot)/lib/rustlib/${host}/bin/llvm-objcopy"
  if [ ! -x "$objcopy" ]; then
    echo "ios-sideload: ${objcopy} is missing. Run: rustup component add llvm-tools" >&2
    exit 1
  fi
  for archive in "$RELEASE"/build/tauri-*/out/swift-rs/Tauri/release/libTauri.a; do
    [ -f "$archive" ] || continue
    "$objcopy" --globalize-symbol=_release_object --globalize-symbol=_retain_object \
      --globalize-symbol=_string_from_bytes "$archive"
  done
  for fingerprint in "$RELEASE"/.fingerprint/tauri-*/lib-tauri; do
    [ -e "$fingerprint" ] && rm -rf "$(dirname "$fingerprint")"
  done
}

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
rm -f "$BUILT"
if ! build 2>&1 | tee "$log"; then
  if ! grep -q '"_string_from_bytes", referenced from' "$log"; then
    exit 1
  fi
  echo "ios-sideload: promoting swift-rs's runtime symbols in libTauri.a, then building again."
  promote_swift_rs_runtime
  build
fi
[ -f "$BUILT" ] || { echo "ios-sideload: the build reported success but wrote no ${BUILT}." >&2; exit 1; }

mkdir -p "$OUT"
cp "$BUILT" "${OUT}/${NAME}.ipa"

# Read what AltStore will read: the app's Info.plist and its signature.
python3 - "${OUT}/${NAME}.ipa" "$IDENTIFIER" "$VERSION" > "${OUT}/${NAME}.json" <<'PY'
import hashlib, json, plistlib, re, subprocess, sys, tempfile, zipfile, os

ipa, identifier, version = sys.argv[1:4]
with zipfile.ZipFile(ipa) as package:
    apps = sorted({name.split("/")[1] for name in package.namelist()
                   if name.startswith("Payload/") and name.count("/") >= 2 and name.split("/")[1].endswith(".app")})
    if len(apps) != 1:
        sys.exit(f"ios-sideload: expected one app in Payload/, found {apps}")
    app = apps[0]
    info = plistlib.loads(package.read(f"Payload/{app}/Info.plist"))
    with tempfile.TemporaryDirectory() as scratch:
        package.extractall(scratch, [name for name in package.namelist() if name.startswith(f"Payload/{app}/")])
        signed = subprocess.run(["codesign", "-d", "--entitlements", ":-", os.path.join(scratch, "Payload", app)],
                                capture_output=True)
        entitlements = plistlib.loads(signed.stdout) if signed.returncode == 0 and signed.stdout.strip() else {}

# The package has to be the edition this script built, or the source would
# advertise one app and install another.
if info.get("CFBundleIdentifier") != identifier:
    sys.exit(f"ios-sideload: the .ipa is {info.get('CFBundleIdentifier')}, not {identifier}")
if info.get("CFBundleShortVersionString") != version:
    sys.exit(f"ios-sideload: the .ipa is version {info.get('CFBundleShortVersionString')}, not {version}")

digest = hashlib.sha256()
with open(ipa, "rb") as stream:
    for block in iter(lambda: stream.read(1 << 20), b""):
        digest.update(block)

print(json.dumps({
    "bundleIdentifier": info["CFBundleIdentifier"],
    "version": info["CFBundleShortVersionString"],
    "buildVersion": str(info.get("CFBundleVersion", info["CFBundleShortVersionString"])),
    "minOSVersion": info.get("MinimumOSVersion"),
    "size": os.path.getsize(ipa),
    "sha256": digest.hexdigest(),
    "entitlements": sorted(entitlements),
    "privacy": {key: value for key, value in sorted(info.items())
                if re.fullmatch(r"NS[A-Za-z]+UsageDescription", key) and isinstance(value, str)},
}, indent=2))
PY

echo "ios-sideload: ${OUT}/${NAME}.ipa"
echo "ios-sideload: ${OUT}/${NAME}.json"

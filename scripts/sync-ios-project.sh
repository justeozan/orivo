#!/usr/bin/env bash
# Bring a generated src-tauri/gen/apple in line with the repository.
#
#   ./scripts/sync-ios-project.sh
#
# `tauri ios init` writes that project once, from Tauri's own templates, and the
# directory is gitignored, so two things in it are wrong for Orivo until this
# runs:
#
#   The bundle identifier. iOS builds are `run.evey.orivo`, the identifier of
#   the App Store Connect record (src-tauri/tauri.ios.conf.json). A project
#   generated before that override still says `io.orivo.desktop`, and a build
#   carrying it uploads to no record at all. Desktop and Android keep
#   `io.orivo.desktop`: it names the data directory and the Android package
#   that existing installs live under, so renaming it there would hand every
#   one of them an empty library.
#
#   The app icon. Init fills the asset catalog with Tauri's logo, and the App
#   Store icon is the one Xcode compiles from that catalog. The committed set in
#   src-tauri/icons/ios is Orivo's ring on the app's dark ground, as RGB: App
#   Store Connect rejects an icon with an alpha channel even when every pixel
#   is opaque (ITMS-90717), so neither the transparent desktop icon nor the
#   RGBA files `tauri icon` writes can be used as they are.
#
# Safe to run any number of times; run-ios-simulator.sh runs it on every start.
set -euo pipefail

cd "$(dirname "$0")/.."
APPLE="src-tauri/gen/apple"
if [ ! -d "$APPLE" ]; then
  echo "ios: ${APPLE} is missing (it is gitignored). Run: pnpm tauri ios init --ci" >&2
  exit 1
fi

IOS_ID="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["identifier"])' \
  src-tauri/tauri.ios.conf.json)"

# Single-quoted perl programs, for the reason run-ios-simulator.sh gives: in
# double quotes bash expands `$1` before perl ever sees it.
perl -pi -e 's/^(  bundleIdPrefix: ).*$/${1}'"$IOS_ID"'/; s/^(      PRODUCT_BUNDLE_IDENTIFIER: ).*$/${1}'"$IOS_ID"'/' \
  "${APPLE}/project.yml"
perl -pi -e 's/(\bPRODUCT_BUNDLE_IDENTIFIER = )[^;]*;/${1}'"$IOS_ID"';/g' \
  "${APPLE}/orivo.xcodeproj/project.pbxproj"

cp src-tauri/icons/ios/AppIcon-*.png "${APPLE}/Assets.xcassets/AppIcon.appiconset/"

echo "ios: bundle identifier ${IOS_ID}, Orivo app icon in place."

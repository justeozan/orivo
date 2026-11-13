#!/usr/bin/env bash
# Builds, signs and packages the Mac App Store edition (docs/app-store.md).
#
#   ORIVO_MAS_PROFILE=<Mac App Store .provisionprofile> scripts/build-mac-app-store.sh
#
# VITE_SENTRY_DSN (and SENTRY_AUTH_TOKEN, for source maps) are read from the
# environment like every release build. Leave the APPLE_API_* and APPLE_ID
# variables unset: with them Tauri would try to notarize, which an App Store
# build does not go through.
set -euo pipefail
: "${ORIVO_MAS_PROFILE:?path to the Mac App Store provisioning profile}"
target="${ORIVO_MAS_TARGET:-aarch64-apple-darwin}"
app_identity="${ORIVO_MAS_APP_IDENTITY:-Apple Distribution}"
installer_identity="${ORIVO_MAS_INSTALLER_IDENTITY:-3rd Party Mac Developer Installer}"
cd "$(dirname "$0")/.."

profile_config=$(printf '{"bundle":{"macOS":{"files":{"embedded.provisionprofile":"%s"}}}}' "$ORIVO_MAS_PROFILE")
APPLE_SIGNING_IDENTITY="$app_identity" pnpm tauri build --target "$target" --bundles app \
  --features app-store --config src-tauri/tauri.appstore.conf.json --config "$profile_config" "$@"

app="target/$target/release/bundle/macos/Orivo.app"
version=$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")
pkg="target/$target/release/bundle/Orivo-$version-app-store.pkg"
productbuild --component "$app" /Applications --sign "$installer_identity" "$pkg"
echo "$pkg"

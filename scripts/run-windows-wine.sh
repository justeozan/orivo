#!/usr/bin/env bash
# Cross-compile the Windows (x86_64) build of Orivo with cargo-xwin, then
# launch it under Wine. macOS-only, meant for local Conductor workspaces.
#
# One-time prerequisites:
#   rustup target add x86_64-pc-windows-msvc   (auto-adds below if missing)
#   LLVM toolchain — brew install llvm lld      (auto-installs below; provides
#     clang-cl, llvm-lib, llvm-rc and lld-link that cargo-xwin/cc-rs expect)
#   cargo-xwin                                  (auto-installs below)
#   Wine Staging — brew install --cask wine-staging
#
# Caveat: on Windows Tauri renders through the WebView2 runtime, which only
# partially works under Wine. The binary launches and exercises the Windows
# code paths (auth stores, wasm runner, IPC, tray), but the window content can
# be blank unless the WebView2 Evergreen runtime is installed into the prefix
# (`winetricks webview2` — attempted below when winetricks is available).
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

TARGET="x86_64-pc-windows-msvc"
EXE=""
WINE_PREFIX="${WINE_PREFIX:-${HOME}/.wine-orivo}"

# 1. Locate Wine: WineHQ app builds first, then whatever is on PATH.
WINE_BIN=""
for candidate in \
  "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine" \
  "/Applications/Wine.app/Contents/Resources/wine/bin/wine" \
  "$(command -v wine 2>/dev/null || true)"; do
  if [ -n "$candidate" ] && [ -x "$candidate" ]; then
    WINE_BIN="$candidate"
    break
  fi
done
if [ -z "$WINE_BIN" ]; then
  echo "wine: not found." >&2
  echo "  Install Wine Staging: brew install --cask wine-staging" >&2
  echo "  or from https://winestaging.dev" >&2
  exit 1
fi
echo "wine: ${WINE_BIN}"

# 2. Rust-side cross toolchain. cargo-xwin downloads the MSVC SDK/CRT itself
#    and drives clang-cl/lld-link/llvm-lib, which a full LLVM install provides.
#    rustup's llvm-tools-preview does NOT ship llvm-lib/llvm-rc, so a plain
#    `clang` on PATH is not enough — ring's C build needs `llvm-lib` too.
rustup target add "$TARGET" >/dev/null
if ! command -v cargo-xwin >/dev/null 2>&1; then
  echo "cargo-xwin: not found, installing (one-time, takes a few minutes)..."
  cargo install --locked cargo-xwin
fi
if ! command -v llvm-lib >/dev/null 2>&1 || ! command -v lld-link >/dev/null 2>&1; then
  echo "llvm-lib/lld-link: not found, installing LLVM + LLD via Homebrew..."
  brew install llvm lld
fi
# Homebrew LLVM is keg-only, so its tools are not on PATH by default.
if [ -d "/opt/homebrew/opt/llvm/bin" ]; then
  export PATH="/opt/homebrew/opt/llvm/bin:$PATH"
fi
if ! command -v llvm-lib >/dev/null 2>&1; then
  echo "llvm-lib: not on PATH after install — set PATH to the LLVM bin dir." >&2
  exit 1
fi

# 3. Build the exe (frontend + Rust). with-signing-key.sh lets bundling work
#    when the updater key is present; --no-bundle produces no updater artifact.
#    The cargo workspace lives at the repo root, so the exe lands in the root
#    target dir (this repo's Cargo.toml is a workspace with member src-tauri).
echo "Building ${TARGET} (frontend + Rust)..."
./scripts/with-signing-key.sh pnpm tauri build \
  --runner cargo-xwin --target "$TARGET" --no-bundle

for candidate in \
  "target/${TARGET}/release/orivo.exe" \
  "src-tauri/target/${TARGET}/release/orivo.exe"; do
  if [ -f "$ROOT/$candidate" ]; then
    EXE="$ROOT/$candidate"
    break
  fi
done
if [ -z "$EXE" ]; then
  echo "orivo.exe not found under target/${TARGET}/release." >&2
  exit 1
fi
echo "exe: ${EXE}"

# 4. Initialise a dedicated Wine prefix (first run only).
export WINEPREFIX="$WINE_PREFIX"
export WINEARCH="win64"
export WINEDEBUG="${WINEDEBUG:--all}"
mkdir -p "$WINE_PREFIX"
if [ ! -f "$WINE_PREFIX/.initialised" ]; then
  echo "wine: initialising prefix ${WINE_PREFIX}..."
  "$WINE_BIN" wineboot -u >/dev/null 2>&1 || true
  touch "$WINE_PREFIX/.initialised"
fi

# 5. Provision WebView2 so the UI has a chance to render. Modern winetricks
#    dropped the `webview2` verb, so the Evergreen Standalone Installer is run
#    directly; success is judged by the runtime version in the registry.
WEBVIEW2_KEY='HKLM\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
installed_webview2() {
  "$WINE_BIN" reg query "$WEBVIEW2_KEY" /v pv >/dev/null 2>&1
}
if ! installed_webview2; then
  echo "wine: installing WebView2 Evergreen Runtime into the prefix (one-time)..."
  SETUP_EXE="$HOME/.cache/orivo/WebView2Setup.exe"
  mkdir -p "$(dirname "$SETUP_EXE")"
  if [ ! -f "$SETUP_EXE" ]; then
    echo "  downloading from go.microsoft.com/fwlink/p/?LinkId=2124703..."
    curl -fsSL -o "$SETUP_EXE" "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
  fi
  "$WINE_BIN" "$SETUP_EXE" /silent /install >/dev/null 2>&1 &
  # Poll the registry for ~5 minutes while the bootstrapper downloads+installs.
  for _ in $(seq 1 60); do
    if installed_webview2; then break; fi
    sleep 5
  done
  if ! installed_webview2; then
    echo "  (WebView2 install failed — the window may stay blank, the exe still runs)" >&2
    echo "  Try: $WINE_BIN $SETUP_EXE /silent /install" >&2
  else
    echo "  WebView2 Runtime installed: $("$WINE_BIN" reg query "$WEBVIEW2_KEY" /v pv 2>/dev/null | awk '/pv/{print $NF}')"
  fi
fi
if installed_webview2; then
  WEBVIEW2_VERSION="$("$WINE_BIN" reg query "$WEBVIEW2_KEY" /v pv 2>/dev/null | awk '/pv/{print $NF}')"
else
  WEBVIEW2_VERSION="not installed"
fi

# 6. WebView2 under Wine sporadically faults in ole32 when the compositor
#    drives GPU work; software compositing keeps it stable (still animates).
export WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="${WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:---disable-gpu}"

echo "Launching Orivo (Windows, x86_64) under Wine..."
echo "  exe:       ${EXE}"
echo "  prefix:    ${WINE_PREFIX}"
echo "  webview2:  ${WEBVIEW2_VERSION:-not installed}"
echo "  browser:   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=${WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS}"
echo "  Tip: Wine+WebView2 stays experimental; if the window is blank, the run"
echo "      still exercises the real Windows code paths below the UI."
exec "$WINE_BIN" "$EXE" "$@"
#!/usr/bin/env bash
# Run a command with the updater signing key in the environment.
#
# `bundle.createUpdaterArtifacts` is on, so every `tauri build` signs the
# updater archive and fails outright when `TAURI_SIGNING_PRIVATE_KEY` is unset.
# The key is deliberately never committed, and `.context/` does not survive a
# workspace being archived, so look for it in the durable location first.
#
#   ./scripts/with-signing-key.sh pnpm tauri build --debug --bundles app
#
# If no key is found the command still runs: `tauri dev`, `pnpm test` and any
# non-bundling task do not need it. See docs/RELEASING.md.
set -euo pipefail

for candidate in "${HOME}/.orivo/orivo-updater.key" ".context/orivo-updater.key"; do
  if [ -r "${candidate}" ]; then
    TAURI_SIGNING_PRIVATE_KEY="$(cat "${candidate}")"
    export TAURI_SIGNING_PRIVATE_KEY
    # The key was generated without a password, but the bundler prompts
    # interactively when the variable is missing entirely.
    export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
    break
  fi
done

if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  echo "with-signing-key: no updater key found, running unsigned." >&2
  echo "  expected ~/.orivo/orivo-updater.key — see docs/RELEASING.md" >&2
fi

exec "$@"

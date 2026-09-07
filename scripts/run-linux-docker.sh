#!/usr/bin/env bash
# Build the Linux (arm64) build of Orivo inside a Docker container, then run
# it with the window forwarded through XQuartz. macOS (Apple Silicon) only,
# meant for local Conductor workspaces.
#
# One-time prerequisites:
#   brew install --cask xquartz     (X server — installed automatically below)
#   Docker Desktop                   (docker CLI must already be installed)
#
# The image is built once (the first run compiles the whole Linux binary,
# allow ~15–20 min) and reused afterwards. Re-running rebuilds the image so
# workspace changes are picked up.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
IMAGE="orivo-linux-test:latest"
CONTAINER_NAME="orivo-linux-test"

# 1. XQuartz — the X server the containerised GUI draws into.
if ! command -v xhost >/dev/null 2>&1; then
  echo "xquartz: not found, installing..."
  brew install --cask xquartz || true
fi
if ! xhost >/dev/null 2>&1; then
  echo "xquartz: starting..."
  open -a XQuartz
  for _ in $(seq 1 30); do
    xhost >/dev/null 2>&1 && break
    sleep 1
  done
fi
xhost +localhost >/dev/null 2>&1 || true

# 2. Docker — the Linux build+run environment.
if ! command -v docker >/dev/null 2>&1; then
  echo "docker: CLI not found. Install Docker Desktop first." >&2
  exit 1
fi
if ! docker info >/dev/null 2>&1; then
  echo "docker: daemon not running, starting Docker Desktop..."
  open -a Docker
  until docker info >/dev/null 2>&1; do sleep 2; done
fi
docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true

# 3. Build the image (incremental; full compile on first run).
echo "Building ${IMAGE} ..."
docker build --platform linux/arm64 -f scripts/orivo-linux-test.Dockerfile -t "$IMAGE" "$ROOT"

# 4. Run it with the host display forwarded.
echo "Launching Orivo (Linux, arm64) through XQuartz..."
docker run --rm --name "$CONTAINER_NAME" \
  -e DISPLAY=host.docker.internal:0 \
  -e GDK_BACKEND=x11 \
  -e WEBKIT_DISABLE_COMPOSITING_MODE=1 \
  -e LIBGL_ALWAYS_SOFTWARE=1 \
  --shm-size=256m \
  "$IMAGE"
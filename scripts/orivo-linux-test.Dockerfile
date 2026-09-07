# Orivo Linux test image.
#
# Builds and runs the Linux (arm64) build of Orivo inside a container, with
# the app window forwarded over X11 to the macOS host (XQuartz). See
# scripts/run-linux-docker.sh for the launcher. This is only for local testing
# on an Apple Silicon Mac; releases are built by CI on native Linux runners.
#
# The first build compiles the entire Rust workspace, so allow some time.

FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

# System libraries needed to build and run a Tauri (WebKitGTK) app.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        dbus \
        file \
        git \
        gnome-keyring \
        libappindicator3-dev \
        libgtk-3-dev \
        librsvg2-dev \
        libssl-dev \
        libwebkit2gtk-4.1-dev \
        libxdo-dev \
        patchelf \
        pkg-config \
        xdg-utils \
    && rm -rf /var/lib/apt/lists/*

# Node 22 + pnpm (matches package.json / CI).
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && npm install -g pnpm@11.8.0

# Rust stable (the repo does not pin a toolchain file).
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH="/usr/local/cargo/bin:${PATH}"
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal

WORKDIR /orivo

# Frontend dependencies — cached until the lockfile changes.
COPY pnpm-lock.yaml pnpm-workspace.yaml package.json ./
RUN pnpm install --frozen-lockfile

# The rest of the repo, then the frontend and the native binary
# (beforeBuildCommand builds ./dist; the backend embeds it).
COPY . .
RUN pnpm tauri build --no-bundle

# Run under a dbus session so the keyring / Secret Service provider has a
# bus to talk to; start the daemon with an empty password for headless use.
CMD ["sh", "-c", \
  "dbus-run-session -- \
     sh -c 'echo -n | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>&1 || true; \
            /orivo/src-tauri/target/release/orivo'"]
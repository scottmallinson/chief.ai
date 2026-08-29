#!/usr/bin/env bash
# Prepare a fresh checkout so linters, tests and the Tauri build can run.
# Best-effort: never fail the session if a step is unavailable.
set -u

log() { printf '[setup] %s\n' "$1"; }

# WebKitGTK toolchain — required to compile the Tauri shell on Linux.
if [ "$(uname -s)" = "Linux" ] && command -v apt-get > /dev/null 2>&1; then
  if ! pkg-config --exists webkit2gtk-4.1 2> /dev/null; then
    log "installing Linux build dependencies"
    SUDO=""
    [ "$(id -u)" -ne 0 ] && command -v sudo > /dev/null 2>&1 && SUDO="sudo"
    DEBIAN_FRONTEND=noninteractive $SUDO apt-get update -qq > /dev/null 2>&1 \
      && DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
        libayatana-appindicator3-dev librsvg2-dev patchelf > /dev/null 2>&1 \
      || log "could not install Linux build dependencies (the frontend still works)"
  fi
fi

if command -v corepack > /dev/null 2>&1; then
  corepack enable > /dev/null 2>&1 || true
fi

if command -v pnpm > /dev/null 2>&1; then
  log "installing node dependencies"
  pnpm install --frozen-lockfile || pnpm install || log "pnpm install failed"
else
  log "pnpm not found — run 'corepack enable' first"
fi

# Disk headroom. A debug build of this crate is several gigabytes and a release
# build more, and cargo reports exhaustion as `failed to build archive ... No
# space left on device` part-way through a link — which reads as a build error
# rather than a full disk. Measured: `src-tauri/target` reached 13 GB during one
# session and took the volume to 99%.
FREE_MB=$(df -Pm . 2>/dev/null | awk 'NR==2 {print $4}')
if [ -n "${FREE_MB:-}" ] && [ "$FREE_MB" -lt 12000 ] 2> /dev/null; then
  log "only ${FREE_MB}MB free — a Tauri build needs several GB"
  if [ -d src-tauri/target ]; then
    log "src-tauri/target is $(du -sh src-tauri/target 2>/dev/null | cut -f1); 'cargo clean --manifest-path src-tauri/Cargo.toml -p Chief' frees most of it"
  fi
fi

log "ready"
exit 0

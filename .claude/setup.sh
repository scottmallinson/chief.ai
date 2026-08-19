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

log "ready"
exit 0

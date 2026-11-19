#!/bin/bash
# Builds the Rust half of Aleno (terminal engine + client FFI static library + daemon).
set -euo pipefail
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# Xcode sets SDKROOT/MACOSX_DEPLOYMENT_TARGET; keep cargo consistent with the app.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-15.0}"
unset SDKROOT 2>/dev/null || true
cargo build --release --locked -p aleno-client -p aleno-daemon --target-dir target 2>&1 | tail -5 || \
cargo build --release -p aleno-client -p aleno-daemon

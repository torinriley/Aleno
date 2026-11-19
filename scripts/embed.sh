#!/bin/bash
# Copies the daemon binary and JSON schema into the app bundle.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$TARGET_BUILD_DIR/$FULL_PRODUCT_NAME"
cp -f "$ROOT/target/release/aleno-daemon" "$APP/Contents/MacOS/aleno-daemon"
mkdir -p "$APP/Contents/Resources"
cp -f "$ROOT/schemas/settings.schema.json" "$APP/Contents/Resources/settings.schema.json"
# Re-sign after modifying the bundle (ad-hoc).
codesign --force --sign - "$APP/Contents/MacOS/aleno-daemon" >/dev/null 2>&1 || true
codesign --force --sign - "$APP" >/dev/null 2>&1 || true

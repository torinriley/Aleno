#!/bin/bash
# Copies the daemon binary and JSON schema into the app bundle.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$TARGET_BUILD_DIR/$FULL_PRODUCT_NAME"
cp -f "$ROOT/target/release/aleno-daemon" "$APP/Contents/MacOS/aleno-daemon"
mkdir -p "$APP/Contents/Resources"
cp -f "$ROOT/schemas/settings.schema.json" "$APP/Contents/Resources/settings.schema.json"
# Build number = number of commits, so every build of a given version is distinguishable and the About window is exact.
# (Falls back to 1 when there is no git history, for example in a source archive.)
BUILD=$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD" "$APP/Contents/Info.plist" 2>/dev/null || true
# Re-sign after modifying the bundle (ad-hoc).
codesign --force --sign - "$APP/Contents/MacOS/aleno-daemon" >/dev/null 2>&1 || true
codesign --force --sign - "$APP" >/dev/null 2>&1 || true

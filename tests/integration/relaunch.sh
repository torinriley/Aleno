#!/bin/bash
# Uses an isolated daemon/socket; does not touch the user's sessions.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$PWD"
TEMP_DIR=$(mktemp -d /tmp/aleno-relaunch.XXXXXX)
export ALENO_DAEMON_PATH="$ROOT/target/release/aleno-daemon"
trap 'rm -rf "$TEMP_DIR"' EXIT
cp -R build/Build/Products/Release/Aleno.app "$TEMP_DIR/Test.app"
SOURCES=()
while IFS= read -r f; do SOURCES+=("$f"); done < <(find app -name '*.swift' ! -name main.swift)
xcrun swiftc -O -target arm64-apple-macos15.0 -import-objc-header app/Application/aleno.h \
  "${SOURCES[@]}" tests/integration/relaunch.swift target/release/libaleno_client.a \
  -framework AppKit -framework Metal -framework MetalKit -framework CoreText -framework QuartzCore \
  -framework UserNotifications -framework Security -o "$TEMP_DIR/Test.app/Contents/MacOS/Aleno"
"$TEMP_DIR/Test.app/Contents/MacOS/Aleno"

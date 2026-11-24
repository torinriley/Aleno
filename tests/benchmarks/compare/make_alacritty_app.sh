#!/bin/bash
# Builds Alacritty from source and wraps it in a minimal app bundle at /tmp/aleno-bench/Alacritty.app, which run.py
# launches like the other terminals. Homebrew's Alacritty cask is disabled (the official build fails Gatekeeper), and a
# locally built binary needs no Gatekeeper exemption.
set -euo pipefail
ROOT=/tmp/aleno-bench
cargo install alacritty --locked --root "$ROOT/alacritty-root"
APP="$ROOT/Alacritty.app"
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS"
cp "$ROOT/alacritty-root/bin/alacritty" "$APP/Contents/MacOS/alacritty"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>alacritty</string>
<key>CFBundleIdentifier</key><string>local.bench.alacritty</string>
<key>CFBundleName</key><string>Alacritty</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>local build</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$APP"
echo "built $APP"

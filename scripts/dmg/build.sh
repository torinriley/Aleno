#!/bin/bash
# Builds the styled disk image: scripts/dmg/build.sh <path/to/Aleno.app> <output.dmg>
# Renders the window background (light, with an arrow between the app and the Applications folder), then lays the
# window out with dmgbuild, which writes the Finder view settings directly, so it also works headless on CI.
# dmgbuild is installed into build/dmg-venv on first use.
set -euo pipefail
cd "$(dirname "$0")/../.."
APP="${1:?usage: build.sh <Aleno.app> <output.dmg>}"
DMG="${2:?usage: build.sh <Aleno.app> <output.dmg>}"
WORK=build/dmg; rm -rf "$WORK"; mkdir -p "$WORK"

if [ ! -x build/dmg-venv/bin/dmgbuild ]; then
  python3 -m venv build/dmg-venv
  build/dmg-venv/bin/pip install -q dmgbuild
fi

swiftc -O scripts/dmg/render-background.swift -o "$WORK/render-background"
"$WORK/render-background" "$WORK" >/dev/null
tiffutil -cathidpicheck "$WORK/background.png" "$WORK/background@2x.png" -out "$WORK/background.tiff" 2>/dev/null   # 1x and 2x in one file

rm -f "$DMG"
build/dmg-venv/bin/dmgbuild -s scripts/dmg/settings.py -D app="$APP" -D background="$WORK/background.tiff" Aleno "$DMG"

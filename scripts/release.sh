#!/bin/bash
# Build Aleno in Release and package it into dist/:
#   Aleno.dmg            disk image with an Applications shortcut
#   Aleno-<version>-arm64.zip   the app bundle
#   SHA256SUMS           checksums for both
#
# Default: sign with a Developer ID certificate (hardened runtime), notarize with Apple and staple the tickets.
# With --unsigned: skip signing and notarization (ad-hoc signed, for CI runs without secrets and local testing).
#
# Signing identity and notarization credentials come from the environment or from config/release.local.env (git-ignored):
#   ALENO_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   ALENO_NOTARY_PROFILE="aleno-notary"          # a `notarytool store-credentials` profile, or instead:
#   ALENO_NOTARY_APPLE_ID, ALENO_NOTARY_PASSWORD (app-specific password), ALENO_NOTARY_TEAM_ID
# See docs/RELEASING.md.
set -euo pipefail
cd "$(dirname "$0")/.."

UNSIGNED=0
[ "${1:-}" = "--unsigned" ] && UNSIGNED=1
[ -f config/release.local.env ] && source config/release.local.env

if [ "$UNSIGNED" = 0 ]; then
  : "${ALENO_SIGN_IDENTITY:?set ALENO_SIGN_IDENTITY (see the comment at the top of this script)}"
  if [ -n "${ALENO_NOTARY_APPLE_ID:-}" ]; then
    NOTARY=(--apple-id "$ALENO_NOTARY_APPLE_ID" --password "${ALENO_NOTARY_PASSWORD:?}" --team-id "${ALENO_NOTARY_TEAM_ID:?}")
  else
    NOTARY=(--keychain-profile "${ALENO_NOTARY_PROFILE:-aleno-notary}")
  fi
fi

OUT=dist
APP="$OUT/Aleno.app"

echo "==> build (Release)"
scripts/generate-project.sh >/dev/null
xcodebuild -project Aleno.xcodeproj -scheme Aleno -configuration Release -derivedDataPath build build 2>&1 | grep -E "error: |\*\* BUILD" | tee /tmp/aleno-build.log
grep -q "BUILD SUCCEEDED" /tmp/aleno-build.log
rm -rf "$OUT"; mkdir -p "$OUT"
ditto build/Build/Products/Release/Aleno.app "$APP"
VERSION=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$APP/Contents/Info.plist")
ZIP="$OUT/Aleno-$VERSION-arm64.zip"
DMG="$OUT/Aleno.dmg"

make_dmg() {
  local stage="$OUT/dmg"; mkdir -p "$stage"
  ditto "$APP" "$stage/Aleno.app"
  ln -s /Applications "$stage/Applications"
  hdiutil create -volname Aleno -srcfolder "$stage" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$stage"
}

if [ "$UNSIGNED" = 1 ]; then
  echo "==> unsigned build: ad-hoc signature only, not notarized"
  codesign --force --sign - "$APP/Contents/MacOS/aleno-daemon"
  codesign --force --sign - "$APP"
  make_dmg
  ditto -c -k --keepParent "$APP" "$ZIP"
else
  echo "==> sign with: $ALENO_SIGN_IDENTITY"
  # Inside-out: nested executables first, then the bundle.
  find "$APP/Contents" -type f \( -perm -111 -o -name '*.dylib' \) ! -path "$APP/Contents/MacOS/Aleno" -print0 |
    while IFS= read -r -d '' f; do
      if file "$f" | grep -q "Mach-O"; then
        codesign --force --timestamp --options runtime --sign "$ALENO_SIGN_IDENTITY" "$f"
      fi
    done
  codesign --force --timestamp --options runtime --sign "$ALENO_SIGN_IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"

  echo "==> notarize the app"
  ditto -c -k --keepParent "$APP" "$OUT/notarize.zip"
  xcrun notarytool submit "$OUT/notarize.zip" "${NOTARY[@]}" --wait
  xcrun stapler staple "$APP"
  rm -f "$OUT/notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"      # the zip carries the stapled app

  echo "==> package and notarize the dmg"
  make_dmg
  codesign --force --timestamp --sign "$ALENO_SIGN_IDENTITY" "$DMG"
  xcrun notarytool submit "$DMG" "${NOTARY[@]}" --wait
  xcrun stapler staple "$DMG"

  echo "==> verify"
  spctl -a -vv -t exec "$APP"
  spctl -a -vv -t open --context context:primary-signature "$DMG"
  xcrun stapler validate "$APP"
fi

( cd "$OUT" && shasum -a 256 "$(basename "$DMG")" "$(basename "$ZIP")" > SHA256SUMS && cat SHA256SUMS )
echo "done: $DMG  $ZIP"

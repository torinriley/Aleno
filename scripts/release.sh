#!/bin/bash
# Build Aleno in Release and package it into dist/:
#   Aleno.dmg            disk image with an Applications shortcut
#   Aleno-<version>-arm64.zip   the app bundle
#   appcast.xml          the Sparkle update feed for this release (signed builds only, needs the Sparkle key)
#   SHA256SUMS           checksums for both
#
# Default: sign with a Developer ID certificate (hardened runtime), notarize with Apple and staple the tickets.
# With --unsigned: skip signing and notarization (ad-hoc signed, for CI runs without secrets and local testing).
#
# Signing identity and notarization credentials come from the environment or from config/release.local.env (git-ignored):
#   ALENO_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   ALENO_NOTARY_PROFILE="aleno-notary"          # a `notarytool store-credentials` profile, or instead:
#   ALENO_NOTARY_APPLE_ID, ALENO_NOTARY_PASSWORD (app-specific password), ALENO_NOTARY_TEAM_ID
# The appcast is signed with the Sparkle EdDSA private key, taken from ALENO_SPARKLE_KEY_FILE (a file holding the exported
# key, as CI provides it) or else from the login keychain, where Sparkle's generate_keys put it.
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
  ditto -c -k --keepParent --norsrc --noextattr "$APP" "$ZIP"
else
  echo "==> sign with: $ALENO_SIGN_IDENTITY"
  # Inside-out: nested code first, then the bundle. Sparkle's helpers are signed in the order Sparkle documents,
  # innermost first: XPC services, Updater.app, Autoupdate, then the framework itself.
  SPARKLE="$APP/Contents/Frameworks/Sparkle.framework"
  if [ -d "$SPARKLE" ]; then
    B="$SPARKLE/Versions/B"
    for x in "$B"/XPCServices/*.xpc "$B/Updater.app" "$B/Autoupdate" "$SPARKLE"; do
      [ -e "$x" ] && codesign --force --timestamp --options runtime --preserve-metadata=entitlements --sign "$ALENO_SIGN_IDENTITY" "$x"
    done
  fi
  find "$APP/Contents" -type f \( -perm -111 -o -name '*.dylib' \) ! -path "$APP/Contents/MacOS/Aleno" ! -path "*/Sparkle.framework/*" -print0 |
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
  ditto -c -k --keepParent --norsrc --noextattr "$APP" "$ZIP"      # the zip carries the stapled app

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

if [ "$UNSIGNED" = 0 ]; then
  echo "==> appcast"
  # Sparkle's tools come with the package resolved by xcodebuild.
  SPARKLE_BIN="build/SourcePackages/artifacts/sparkle/Sparkle/bin"
  STAGE="$OUT/appcast-stage"; mkdir -p "$STAGE"
  cp "$ZIP" "$STAGE/"
  KEY_ARGS=()
  [ -n "${ALENO_SPARKLE_KEY_FILE:-}" ] && KEY_ARGS=(--ed-key-file "$ALENO_SPARKLE_KEY_FILE")
  "$SPARKLE_BIN/generate_appcast" ${KEY_ARGS[@]+"${KEY_ARGS[@]}"} \
    --download-url-prefix "https://github.com/torinriley/Aleno/releases/download/v$VERSION/" \
    --full-release-notes-url "https://github.com/torinriley/Aleno/releases/tag/v$VERSION" \
    --link "https://github.com/torinriley/Aleno" \
    -o "$OUT/appcast.xml" "$STAGE"
  rm -rf "$STAGE"
  grep -q "sparkle:edSignature" "$OUT/appcast.xml" || { echo "appcast is not signed: is the Sparkle key available?" >&2; exit 1; }
fi

( cd "$OUT" && shasum -a 256 "$(basename "$DMG")" "$(basename "$ZIP")" > SHA256SUMS && cat SHA256SUMS )
echo "done: $DMG  $ZIP"

#!/bin/bash
# Build Aleno in Release, sign it with the Developer ID certificate (hardened runtime), notarize it with Apple,
# staple the ticket and package a notarized .dmg in dist/.
#
# One-time setup: a "Developer ID Application" certificate in the login keychain, and notarization credentials:
#   xcrun notarytool store-credentials aleno-notary --apple-id <apple id> --team-id <team id>
set -euo pipefail
cd "$(dirname "$0")/.."

# Your signing identity and notarytool profile come from the environment or from config/release.local.env (git-ignored):
#   ALENO_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   ALENO_NOTARY_PROFILE="aleno-notary"
[ -f config/release.local.env ] && source config/release.local.env
: "${ALENO_SIGN_IDENTITY:?set ALENO_SIGN_IDENTITY (see the comment at the top of this script)}"
IDENTITY="$ALENO_SIGN_IDENTITY"
PROFILE="${ALENO_NOTARY_PROFILE:-aleno-notary}"
OUT=dist
APP="$OUT/Aleno.app"

echo "==> build (Release)"
scripts/generate-project.sh >/dev/null
xcodebuild -project Aleno.xcodeproj -scheme Aleno -configuration Release -derivedDataPath build build 2>&1 | grep -E "error: |\*\* BUILD" || true
rm -rf "$OUT"; mkdir -p "$OUT"
ditto build/Build/Products/Release/Aleno.app "$APP"

echo "==> sign with: $IDENTITY"
# Inside-out: nested executables first, then the bundle.
find "$APP/Contents" -type f \( -perm -111 -o -name '*.dylib' \) ! -path "$APP/Contents/MacOS/Aleno" -print0 |
  while IFS= read -r -d '' f; do
    if file "$f" | grep -q "Mach-O"; then
      codesign --force --timestamp --options runtime --sign "$IDENTITY" "$f"
    fi
  done
codesign --force --timestamp --options runtime --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

echo "==> notarize the app"
ditto -c -k --keepParent "$APP" "$OUT/Aleno-app.zip"
xcrun notarytool submit "$OUT/Aleno-app.zip" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$APP"
rm -f "$OUT/Aleno-app.zip"

echo "==> package and notarize the dmg"
STAGE="$OUT/dmg"; mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Aleno.app"
ln -s /Applications "$STAGE/Applications"
DMG="$OUT/Aleno.dmg"
hdiutil create -volname Aleno -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$DMG"

echo "==> verify"
spctl -a -vv -t exec "$APP"
spctl -a -vv -t open --context context:primary-signature "$DMG"
xcrun stapler validate "$APP"
echo "done: $DMG"

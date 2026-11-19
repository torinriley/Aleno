#!/bin/bash
# Regenerates Aleno.xcodeproj from project.yml. XcodeGen does not know Icon Composer documents and marks AppIcon.icon as
# "wrapper.icon"; Xcode needs "folder.iconcomposer.icon" so that actool compiles it into the app icon (otherwise the
# folder is just copied into the bundle).
set -euo pipefail
cd "$(dirname "$0")/.."
xcodegen generate
sed -i '' 's/lastKnownFileType = wrapper\.icon;/lastKnownFileType = folder.iconcomposer.icon;/' Aleno.xcodeproj/project.pbxproj
grep -q "folder.iconcomposer.icon" Aleno.xcodeproj/project.pbxproj && echo "AppIcon.icon file type patched"

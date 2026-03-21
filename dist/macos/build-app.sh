#!/usr/bin/env bash
set -euo pipefail

# Build Termojinal.app bundle from release binary + resources.
# Usage: ./dist/macos/build-app.sh [--debug]
#
# Code signing:
#   Set CODESIGN_IDENTITY to sign with a Developer ID certificate.
#   Example: CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" ./dist/macos/build-app.sh
#   If not set, the bundle is signed with an ad-hoc signature (--sign -).

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="debug"
fi

# Debug builds use termojinal-dev, release builds use termojinal
if [[ "$PROFILE" == "debug" ]]; then
    BINARY="$REPO_ROOT/target/$PROFILE/termojinal-dev"
else
    BINARY="$REPO_ROOT/target/$PROFILE/termojinal"
fi

if [[ ! -f "$BINARY" ]]; then
    echo "Error: $BINARY not found. Build the binary first." >&2
    exit 1
fi

APP_DIR="$REPO_ROOT/target/$PROFILE/Termojinal.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES"

# Copy binaries into the bundle
cp "$BINARY" "$MACOS_DIR/termojinal"

# Include termojinald so Accessibility permission covers the whole .app
DAEMON="$REPO_ROOT/target/$PROFILE/termojinald"
if [[ -f "$DAEMON" ]]; then
    cp "$DAEMON" "$MACOS_DIR/termojinald"
fi

# Copy Info.plist and patch for debug builds
cp "$REPO_ROOT/dist/macos/Info.plist" "$CONTENTS/Info.plist"
if [[ "$PROFILE" == "debug" ]]; then
    /usr/bin/sed -i '' \
        -e 's/com\.termojinal\.app/com.termojinal.app.dev/' \
        -e 's/<string>Termojinal<\/string>/<string>Termojinal Dev<\/string>/' \
        "$CONTENTS/Info.plist"
fi

# Build .icns from png assets
ICONSET="$REPO_ROOT/target/AppIcon.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"

ASSETS="$REPO_ROOT/resources/Assets.xcassets/AppIcon.appiconset"
cp "$ASSETS/16.png"   "$ICONSET/icon_16x16.png"
cp "$ASSETS/32.png"   "$ICONSET/icon_16x16@2x.png"
cp "$ASSETS/32.png"   "$ICONSET/icon_32x32.png"
cp "$ASSETS/64.png"   "$ICONSET/icon_32x32@2x.png"
cp "$ASSETS/128.png"  "$ICONSET/icon_128x128.png"
cp "$ASSETS/256.png"  "$ICONSET/icon_128x128@2x.png"
cp "$ASSETS/256.png"  "$ICONSET/icon_256x256.png"
cp "$ASSETS/512.png"  "$ICONSET/icon_256x256@2x.png"
cp "$ASSETS/512.png"  "$ICONSET/icon_512x512.png"
cp "$ASSETS/1024.png" "$ICONSET/icon_512x512@2x.png"

iconutil -c icns -o "$RESOURCES/AppIcon.icns" "$ICONSET"
rm -rf "$ICONSET"

# Copy license files for the About screen
cp "$REPO_ROOT/LICENSE" "$RESOURCES/LICENSE"
if [[ -f "$REPO_ROOT/THIRD_PARTY_LICENSES.md" ]]; then
    cp "$REPO_ROOT/THIRD_PARTY_LICENSES.md" "$RESOURCES/THIRD_PARTY_LICENSES.md"
fi

# --- Code sign the .app bundle ---
ENTITLEMENTS="$REPO_ROOT/dist/macos/entitlements.plist"
IDENTITY="${CODESIGN_IDENTITY:--}"

if [[ "$IDENTITY" == "-" ]]; then
    echo "==> Signing with ad-hoc identity (set CODESIGN_IDENTITY for Developer ID)"
else
    echo "==> Signing with: $IDENTITY"
fi

codesign --force --deep --options runtime \
    --entitlements "$ENTITLEMENTS" \
    --sign "$IDENTITY" \
    "$APP_DIR"

echo "==> Built $APP_DIR"

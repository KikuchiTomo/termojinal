#!/usr/bin/env bash
set -euo pipefail

# Sync version across Cargo.toml files, Info.plist, and Homebrew formula.
# Usage: ./dist/set-version.sh 0.1.1-beta
#
# Accepts a version string WITHOUT the "v" prefix.

VERSION="${1:?Usage: set-version.sh <version>}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> Setting version to ${VERSION}"

# 1. Update all Cargo.toml files (workspace root + crates)
for toml in "$REPO_ROOT"/Cargo.toml "$REPO_ROOT"/crates/*/Cargo.toml; do
    /usr/bin/sed -i '' "s/^version = \".*\"/version = \"${VERSION}\"/" "$toml"
done
echo "[ok] Cargo.toml files"

# 2. Update Info.plist
PLIST="$REPO_ROOT/dist/macos/Info.plist"
if [[ -f "$PLIST" ]]; then
    # CFBundleShortVersionString
    /usr/bin/sed -i '' "/<key>CFBundleShortVersionString<\/key>/{n;s|<string>.*</string>|<string>${VERSION}</string>|;}" "$PLIST"
    # CFBundleVersion — use numeric part only (strip -beta etc.)
    NUMERIC_VERSION="${VERSION%%-*}"
    /usr/bin/sed -i '' "/<key>CFBundleVersion<\/key>/{n;s|<string>.*</string>|<string>${NUMERIC_VERSION}</string>|;}" "$PLIST"
    echo "[ok] Info.plist"
fi

# 3. Update Homebrew formula version
FORMULA="$REPO_ROOT/dist/homebrew/termojinal.rb"
if [[ -f "$FORMULA" ]]; then
    /usr/bin/sed -i '' "s/^  version \".*\"/  version \"${VERSION}\"/" "$FORMULA"
    echo "[ok] Homebrew formula"
fi

echo "==> Done: ${VERSION}"

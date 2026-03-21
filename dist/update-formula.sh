#!/usr/bin/env bash
set -euo pipefail

# Update Homebrew formula with version and sha256 values.
# Usage: ./dist/update-formula.sh <version> <cli_sha256> <app_sha256>

VERSION="${1:?Usage: update-formula.sh <version> <cli_sha> <app_sha>}"
CLI_SHA="${2:?}"
APP_SHA="${3:?}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FORMULA="${REPO_ROOT}/dist/homebrew/termojinal.rb"

if [[ ! -f "$FORMULA" ]]; then
    echo "Error: $FORMULA not found" >&2
    exit 1
fi

# Validate inputs are hex-only (defense against injection)
if ! echo "$CLI_SHA" | grep -qE '^[0-9a-f]{64}$'; then
    echo "Error: CLI_SHA is not a valid sha256 hash" >&2
    exit 1
fi
if ! echo "$APP_SHA" | grep -qE '^[0-9a-f]{64}$'; then
    echo "Error: APP_SHA is not a valid sha256 hash" >&2
    exit 1
fi
if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$'; then
    echo "Error: VERSION is not valid semver" >&2
    exit 1
fi

# Use awk for reliable multi-pattern replacement (works on macOS and Linux)
awk -v version="$VERSION" -v cli_sha="$CLI_SHA" -v app_sha="$APP_SHA" '
    /^  version / { print "  version \"" version "\""; next }
    /cli-macos-universal\.tar\.gz/ { print; getline; }
    /macos-universal\.tar\.gz/ && !/cli-/ { print; getline; }
    /# sha256.*# Updated automatically/ {
        if (!cli_done) {
            print "  sha256 \"" cli_sha "\""
            cli_done = 1
        } else {
            print "    sha256 \"" app_sha "\""
        }
        next
    }
    { print }
' "$FORMULA" > "${FORMULA}.tmp" && mv "${FORMULA}.tmp" "$FORMULA"

# Verify both sha256 values were actually written
if grep -q '# sha256 ""' "$FORMULA"; then
    echo "Error: Failed to update sha256 values" >&2
    exit 1
fi

echo "[ok] Formula updated: v${VERSION}"

#!/usr/bin/env bash
set -euo pipefail

# Update Homebrew formula with version and sha256 values.
# Usage: ./dist/update-formula.sh <version> <cli_sha256> <app_sha256>
#
# Replaces version and both sha256 lines. Works on first run (placeholder)
# and on subsequent runs (existing hash values).

VERSION="${1:?Usage: update-formula.sh <version> <cli_sha> <app_sha>}"
CLI_SHA="${2:?}"
APP_SHA="${3:?}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FORMULA="${REPO_ROOT}/dist/homebrew/termojinal.rb"

if [[ ! -f "$FORMULA" ]]; then
    echo "Error: $FORMULA not found" >&2
    exit 1
fi

# Validate inputs (defense against injection)
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

# Strategy: replace version line, then replace sha256 lines by position.
# First sha256 (after cli url) = CLI hash, second sha256 (after app url) = APP hash.
awk -v version="$VERSION" -v cli_sha="$CLI_SHA" -v app_sha="$APP_SHA" '
    /TERMOJINAL_VERSION = / {
        print "  TERMOJINAL_VERSION = \"" version "\""
        next
    }
    /^  version / {
        print "  version TERMOJINAL_VERSION"
        next
    }
    /cli-macos-universal\.tar\.gz/ {
        print
        replace_next = "cli"
        next
    }
    /macos-universal\.tar\.gz/ && !/cli-/ {
        print
        replace_next = "app"
        next
    }
    replace_next == "cli" && /sha256/ {
        print "  sha256 \"" cli_sha "\""
        replace_next = ""
        next
    }
    replace_next == "app" && /sha256/ {
        print "    sha256 \"" app_sha "\""
        replace_next = ""
        next
    }
    { print }
' "$FORMULA" > "${FORMULA}.tmp" && mv "${FORMULA}.tmp" "$FORMULA"

# Verify
if ! grep -q "$CLI_SHA" "$FORMULA"; then
    echo "Error: CLI sha256 not found in formula after update" >&2
    exit 1
fi
if ! grep -q "$APP_SHA" "$FORMULA"; then
    echo "Error: APP sha256 not found in formula after update" >&2
    exit 1
fi

echo "[ok] Formula updated: v${VERSION}"

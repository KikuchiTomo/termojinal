#!/usr/bin/env bash
set -euo pipefail

# Generate THIRD_PARTY_LICENSES.md from cargo metadata.
# Run this before building a release.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$REPO_ROOT/THIRD_PARTY_LICENSES.md"

echo "==> Generating $OUT"

cat > "$OUT" << 'HEADER'
# Third-Party Licenses

Termojinal includes the following third-party open source software.
Each package is listed with its license.

HEADER

cargo metadata --manifest-path "$REPO_ROOT/Cargo.toml" --format-version=1 2>/dev/null | \
python3 -c "
import json, sys

meta = json.load(sys.stdin)
workspace = {p['name'] for p in meta['packages'] if p['source'] is None}

deps = {}
for pkg in meta['packages']:
    if pkg['source'] is not None:
        key = pkg['name']
        if key not in deps or pkg['version'] > deps[key][1]:
            deps[key] = (
                pkg['name'],
                pkg['version'],
                pkg.get('license', 'unknown'),
                pkg.get('repository', pkg.get('homepage', '')),
            )

print('| Package | Version | License | Repository |')
print('|---------|---------|---------|------------|')
for key in sorted(deps.keys(), key=str.lower):
    name, ver, lic, repo = deps[key]
    repo_link = f'[link]({repo})' if repo else ''
    print(f'| {name} | {ver} | {lic} | {repo_link} |')
" >> "$OUT"

echo "" >> "$OUT"
echo "---" >> "$OUT"
echo "" >> "$OUT"
echo "Generated on $(date -u '+%Y-%m-%d') by \`dist/generate-licenses.sh\`." >> "$OUT"

echo "[ok] $(wc -l < "$OUT" | tr -d ' ') lines written"

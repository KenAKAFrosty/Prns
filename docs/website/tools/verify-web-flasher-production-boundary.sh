#!/usr/bin/env bash
set -euo pipefail

website="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(cd "$website/../.." && pwd)"
hosted="${1:-}"
embedded="${2:-}"
fixture_key="$website/web-flasher/browser/fixtures/signed-candidate/minisign.pub"

if [[ -z "$hosted" || -z "$embedded" ]]; then
    echo "usage: tools/verify-web-flasher-production-boundary.sh HOSTED_DIST EMBEDDED_DIST" >&2
    exit 2
fi
for directory in "$hosted" "$embedded"; do
    if [[ ! -f "$directory/index.html" ]]; then
        echo "production-boundary input has no index.html: $directory" >&2
        exit 2
    fi
done

trust_scan=(
    python3
    "$workspace/tools/release/flasher_browser_test_trust.py"
    --fixture-key
    "$fixture_key"
)
if [[ -f "$hosted/source.zip" ]]; then
    trust_scan+=(--allow-exact-blob "$hosted/source.zip")
fi
trust_scan+=("$hosted" "$embedded")
"${trust_scan[@]}"
if grep -n '^default[[:space:]]*=.*browser-test-fixture' "$website/Cargo.toml"; then
    echo "browser-test-fixture cannot be a default website feature" >&2
    exit 1
fi
if grep -R -n -E 'dx build.*browser-test-fixture|--features[^[:cntrl:]]*browser-test-fixture' \
    "$workspace/tools/release/build-flasher-candidate.sh" "$workspace/.github/workflows"; then
    echo "a production build command enables browser-test-fixture" >&2
    exit 1
fi

if find "$embedded" \( -path '*/firmware/*' -o -path '*/assets/flasher/*' \) -print -quit | grep -q .; then
    echo "embedded output contains hosted firmware or flasher assets" >&2
    exit 1
fi
if grep -R -a -l -i -E 'esptool-js|esp-web-install-button|unpkg|prns-flash\.js' "$embedded"; then
    echo "embedded output contains hosted flashing JavaScript" >&2
    exit 1
fi

echo "verified production browser-test trust boundary and embedded flasher exclusion"

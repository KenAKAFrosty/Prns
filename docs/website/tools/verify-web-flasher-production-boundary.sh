#!/usr/bin/env bash
set -euo pipefail

website="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(cd "$website/../.." && pwd)"
hosted="${1:-}"
embedded="${2:-}"
marker='PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1'
fixture_key="$(sed -n '2p' "$website/web-flasher/browser/fixtures/signed-candidate/minisign.pub")"

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

if rg -a -l -F "$marker" "$hosted" "$embedded"; then
    echo "a production output contains the browser-test fixture marker" >&2
    exit 1
fi
if rg -a -l -F "$fixture_key" "$hosted" "$embedded"; then
    echo "a production output contains the browser-test Minisign public key" >&2
    exit 1
fi
if rg -n '^default[[:space:]]*=.*browser-test-fixture' "$website/Cargo.toml"; then
    echo "browser-test-fixture cannot be a default website feature" >&2
    exit 1
fi
if rg -n 'dx build.*browser-test-fixture|--features[^[:cntrl:]]*browser-test-fixture' \
    "$workspace/tools/release/build-flasher-candidate.sh" "$workspace/.github/workflows"; then
    echo "a production build command enables browser-test-fixture" >&2
    exit 1
fi

if find "$embedded" \( -path '*/firmware/*' -o -path '*/assets/flasher/*' \) -print -quit | rg -q .; then
    echo "embedded output contains hosted firmware or flasher assets" >&2
    exit 1
fi
if rg -a -l -i 'esptool-js|esp-web-install-button|unpkg|prns-flash\.js' "$embedded"; then
    echo "embedded output contains hosted flashing JavaScript" >&2
    exit 1
fi

echo "verified production browser-test trust boundary and embedded flasher exclusion"

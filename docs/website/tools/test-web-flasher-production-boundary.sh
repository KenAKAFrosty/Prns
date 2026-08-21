#!/usr/bin/env bash
set -euo pipefail

website="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(cd "$website/../.." && pwd)"
test_root="$website/target/browser-tests"
dioxus_dist="$website/target/dx/reticulum-site/release/web/public"
hosted="$test_root/production-hosted"
embedded="$test_root/production-embedded"
captive_page="$workspace/personal-hopspot/embedded/esp32/assets/captive-portal.html"
public_isolation="$(mktemp -d "${TMPDIR:-/tmp}/prns-web-boundary.XXXXXX")"
hosted_public_paths=(
    "public/firmware"
    "public/assets/flasher"
    "public/flash-manifest.json"
    "public/source.zip"
    "public/source.zip.sha256"
)

require_line() {
    local file="$1"
    local line="$2"
    if ! grep -qxF "$line" "$file"; then
        echo "$file must contain: $line" >&2
        exit 1
    fi
}

restore_hosted_public_paths() {
    local relative original saved generated
    set +e
    for relative in "${hosted_public_paths[@]}"; do
        original="$website/$relative"
        saved="$public_isolation/original/$relative"
        generated="$public_isolation/generated/$relative"
        if [[ -e "$original" || -L "$original" ]]; then
            mkdir -p "$(dirname "$generated")"
            mv -- "$original" "$generated"
        fi
        if [[ -e "$saved" || -L "$saved" ]]; then
            mkdir -p "$(dirname "$original")"
            mv -- "$saved" "$original"
        fi
    done
    rm -rf -- "$public_isolation"
}

trap restore_hosted_public_paths EXIT
trap 'exit 130' INT TERM

for relative in "${hosted_public_paths[@]}"; do
    original="$website/$relative"
    if [[ -e "$original" || -L "$original" ]]; then
        saved="$public_isolation/original/$relative"
        mkdir -p "$(dirname "$saved")"
        mv -- "$original" "$saved"
    fi
done

case "$test_root" in
    "$website/target/browser-tests") ;;
    *) echo "refusing unexpected browser-test output path: $test_root" >&2; exit 2 ;;
esac

mkdir -p "$test_root"
cd "$website"
require_line "$website/tailwind.css" '@import "tailwindcss" source(none);'
require_line "$website/tailwind.css" '@source "./src";'
require_line "$website/tailwind.css" '@source "./index.html";'
require_line "$website/web-flasher/browser/playwright.config.mjs" 'const browserOutput = path.join(websiteRoot, "target/browser-tests");'
require_line "$website/web-flasher/browser/playwright.config.mjs" '      outputFolder: path.join(browserOutput, "report"),'
require_line "$website/web-flasher/browser/playwright.config.mjs" '  outputDir: path.join(browserOutput, "results"),'
npm run build:css
npm run build:flasher

rm -rf -- "$dioxus_dist" "$hosted"
PRNS_BUILD_CHANNEL=stable \
dx build --platform web --debug-symbols false --release --locked
test -f "$dioxus_dist/index.html"
mkdir -p "$hosted"
cp -R "$dioxus_dist/." "$hosted/"
mkdir -p "$hosted/assets/flasher"
cp "$website/target/hosted-assets/prns-flash.js" "$hosted/assets/flasher/prns-flash.js"
cp -R "$website/target/hosted-assets/nrf-dfu" "$hosted/assets/flasher/nrf-dfu"

test -f "$captive_page"
rm -rf -- "$embedded"
mkdir -p "$embedded"
cp "$captive_page" "$embedded/index.html"

invalid_features_log="$test_root/invalid-production-feature-combination.log"
local_key="$website/web-flasher/browser/fixtures/signed-candidate/minisign.pub"
local_digest="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
local_version="0.3.1-dev.clean.$local_digest"
local_commit="0123456789abcdef0123456789abcdef01234567"
if PRNS_BUILD_VERSION="$local_version" \
    PRNS_BUILD_COMMIT="$local_commit" \
    PRNS_LOCAL_DEV_PUBLIC_KEY="$local_key" \
    PRNS_LOCAL_DEV_BOARDS="heltec-v4" \
    PRNS_LOCAL_DEV_SOURCE_DIGEST="$local_digest" \
    PRNS_LOCAL_DEV_SOURCE_STATE="clean" \
    cargo check --locked --features "browser-test-fixture local-dev-flasher" \
    >"$invalid_features_log" 2>&1; then
    echo "invalid website features unexpectedly compiled" >&2
    exit 1
fi
grep -qF 'local-dev-flasher is mutually exclusive with every other website profile' \
    "$invalid_features_log"
if PRNS_LOCAL_DEV_PUBLIC_KEY="$local_key" cargo check --locked \
    >"$invalid_features_log" 2>&1; then
    echo "production website unexpectedly accepted local development build inputs" >&2
    exit 1
fi
grep -qF 'PRNS_LOCAL_DEV_PUBLIC_KEY is forbidden without the local-dev-flasher feature' \
    "$invalid_features_log"

cd "$workspace"
bash "$website/tools/verify-web-flasher-production-boundary.sh" "$hosted" "$embedded"
if find "$embedded" \( -name 'source.zip' -o -name 'source.zip.sha256' \) -print -quit | grep -q .; then
    echo "embedded SoftAP site unexpectedly contains hosted source artifacts" >&2
    exit 1
fi

trust_test="$test_root/archive-aware-trust"
trust_hosted="$trust_test/hosted"
trust_embedded="$trust_test/embedded"
trust_failure="$trust_test/failure.log"
rm -rf -- "$trust_test"
mkdir -p "$trust_hosted" "$trust_embedded"
printf '%s\n' '<html></html>' > "$trust_hosted/index.html"
printf '%s\n' '<html></html>' > "$trust_embedded/index.html"
printf '%s\n' 'PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1' > "$trust_hosted/source.zip"
sed -n '2p' \
    "$website/web-flasher/browser/fixtures/signed-candidate/minisign.pub" \
    >> "$trust_hosted/source.zip"
cp "$trust_hosted/source.zip" "$trust_embedded/source-capable.wasm"
bash "$website/tools/verify-web-flasher-production-boundary.sh" \
    "$trust_hosted" "$trust_embedded"

printf '%s\n' 'PRNS_BROWSER_TEST_FIXTURE_TRUST_ROOT_V1' \
    > "$trust_embedded/raw-marker.wasm"
if bash "$website/tools/verify-web-flasher-production-boundary.sh" \
    "$trust_hosted" "$trust_embedded" > "$trust_failure" 2>&1; then
    echo "production boundary accepted a raw browser-test fixture marker" >&2
    exit 1
fi
grep -qF 'a production output contains the browser-test fixture marker' "$trust_failure"
rm "$trust_embedded/raw-marker.wasm"

sed -n '2p' \
    "$website/web-flasher/browser/fixtures/signed-candidate/minisign.pub" \
    > "$trust_embedded/raw-key.wasm"
if bash "$website/tools/verify-web-flasher-production-boundary.sh" \
    "$trust_hosted" "$trust_embedded" > "$trust_failure" 2>&1; then
    echo "production boundary accepted a raw browser-test Minisign public key" >&2
    exit 1
fi
grep -qF 'a production output contains the browser-test Minisign public key' "$trust_failure"
rm "$trust_embedded/raw-key.wasm"

printf '%s\n' \
    'PRNS_LOCAL_DEV_FLASHER_TRUST_ROOT_V1' \
    'PRNS_LOCAL_DEV_FLASHER_BANNER_V1' \
    'LOCAL DEVELOPER FIRMWARE — EPHEMERALLY SIGNED, NOT A RELEASE' \
    'RWQbcQAOQdNia9cRKsl1wJxV2iODb6aBWOI1G0yDDk4ORXKecWSigfoy' \
    > "$trust_embedded/local-development.wasm"
if bash "$website/tools/verify-web-flasher-production-boundary.sh" \
    "$trust_hosted" "$trust_embedded" > "$trust_failure" 2>&1; then
    echo "production boundary accepted local development trust material" >&2
    exit 1
fi
grep -qF 'a production output contains the local-development trust marker' "$trust_failure"
grep -qF 'a production output contains the local-development banner marker' "$trust_failure"
grep -qF 'a production output contains the local-development banner' "$trust_failure"
grep -qF 'a production output contains the non-production Minisign public key' "$trust_failure"
rm "$trust_embedded/local-development.wasm"

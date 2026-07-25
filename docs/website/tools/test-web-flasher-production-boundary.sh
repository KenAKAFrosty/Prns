#!/usr/bin/env bash
set -euo pipefail

website="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(cd "$website/../.." && pwd)"
test_root="$website/target/browser-tests"
dioxus_dist="$website/target/dx/reticulum-site/release/web/public"
hosted="$test_root/production-hosted"
embedded="$test_root/production-embedded"
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

rm -rf -- "$dioxus_dist" "$embedded"
PRNS_EMBEDDED_SITE=1 \
PRNS_BUILD_CHANNEL=stable \
dx build --platform web --debug-symbols false --release --locked --features embedded-site
test -f "$dioxus_dist/index.html"
mkdir -p "$embedded"
cp -R "$dioxus_dist/." "$embedded/"

invalid_features_log="$test_root/invalid-production-feature-combination.log"
if cargo check --locked --features 'embedded-site browser-test-fixture' >"$invalid_features_log" 2>&1; then
    echo "embedded-site unexpectedly compiled with browser-test-fixture" >&2
    exit 1
fi
if ! grep -qF 'browser-test-fixture is forbidden in embedded production builds' "$invalid_features_log"; then
    echo "the invalid production feature combination failed for an unexpected reason" >&2
    exit 1
fi

cd "$workspace"
bash "$website/tools/verify-web-flasher-production-boundary.sh" "$hosted" "$embedded"
if find "$embedded" \( -name 'source.zip' -o -name 'source.zip.sha256' \) -print -quit | grep -q .; then
    echo "embedded SoftAP site unexpectedly contains hosted source artifacts" >&2
    exit 1
fi

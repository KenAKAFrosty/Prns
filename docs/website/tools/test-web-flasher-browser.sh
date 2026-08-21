#!/usr/bin/env bash
set -euo pipefail

website="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace="$(cd "$website/../.." && pwd)"
test_root="$website/target/browser-tests"
dioxus_dist="$website/target/dx/reticulum-site/release/web/public"
site="$test_root/site"
fixture_manifest="$website/web-flasher/browser/fixtures/signed-candidate/releases/0.2.6/flash-manifest.json"

case "$test_root" in
    "$website/target/browser-tests") ;;
    *) echo "refusing unexpected browser-test output path: $test_root" >&2; exit 2 ;;
esac

fixture_version="$(node -e 'const fs=require("node:fs"); const value=JSON.parse(fs.readFileSync(process.argv[1], "utf8")); process.stdout.write(value.release.version)' "$fixture_manifest")"
fixture_commit="$(node -e 'const fs=require("node:fs"); const value=JSON.parse(fs.readFileSync(process.argv[1], "utf8")); process.stdout.write(value.release.commit)' "$fixture_manifest")"

rm -rf -- "$test_root"
mkdir -p "$site/assets/flasher"
cd "$website"
npm run build:css
npm run build:flasher

rm -rf -- "$dioxus_dist"
PRNS_BUILD_VERSION="$fixture_version" \
PRNS_BUILD_COMMIT="$fixture_commit" \
PRNS_BUILD_CHANNEL=stable \
dx build --platform web --debug-symbols false --release --locked --features browser-test-fixture

test -f "$dioxus_dist/index.html"
cp -R "$dioxus_dist/." "$site/"
cp "$website/target/hosted-assets/prns-flash.js" "$site/assets/flasher/prns-flash.js"
cp "$website/target/hosted-assets/prns-flash.js.map" "$site/assets/flasher/prns-flash.js.map"
cp -R "$website/target/hosted-assets/nrf-dfu" "$site/assets/flasher/nrf-dfu"
node web-flasher/browser/support/build-fixture.mjs "$site"
production_bundle_sha256="$(node -e 'const fs=require("node:fs"); const crypto=require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"))' "$site/assets/flasher/prns-flash.js")"
nrf_dfu_binding_sha256="$(node -e 'const fs=require("node:fs"); const crypto=require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"))' "$site/assets/flasher/nrf-dfu/prns_nrf_dfu_core.js")"
nrf_dfu_core_sha256="$(node -e 'const fs=require("node:fs"); const crypto=require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"))' "$site/assets/flasher/nrf-dfu/prns_nrf_dfu_core_bg.wasm")"

cd "$workspace"
PRNS_EXPECTED_FLASH_BUNDLE_SHA256="$production_bundle_sha256" \
PRNS_EXPECTED_NRF_DFU_BINDING_SHA256="$nrf_dfu_binding_sha256" \
PRNS_EXPECTED_NRF_DFU_CORE_SHA256="$nrf_dfu_core_sha256" \
"$website/node_modules/.bin/playwright" test \
    --config "$website/web-flasher/browser/playwright.config.mjs"

#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo test --locked -p prns-flash-manifest -p hopspot-flash
cargo clippy --locked -p prns-flash-manifest -p hopspot-flash --all-targets -- -D warnings
./tools/prns run build.web-flasher.nrf-dfu.test
cargo fmt --manifest-path docs/website/Cargo.toml -- --check
cargo test --manifest-path docs/website/Cargo.toml --locked
cargo clippy --manifest-path docs/website/Cargo.toml --locked --all-targets -- -D warnings

npm --prefix docs/website ci --ignore-scripts --no-audit --no-fund
if [[ "$(id -u)" -eq 0 ]] || sudo -n true >/dev/null 2>&1; then
    npx --prefix docs/website playwright install --with-deps chromium
else
    npx --prefix docs/website playwright install chromium
fi
npm --prefix docs/website run test:flasher
npm --prefix docs/website run build:flasher
npm --prefix docs/website run build:css
npm --prefix docs/website run test:browser
npm --prefix docs/website run test:production-boundary

echo "FLASHER_WEB_GATE_OK"

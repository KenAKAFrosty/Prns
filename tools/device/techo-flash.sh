#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VOLUME="${HOPSPOT_TECHOBOOT:-/Volumes/TECHOBOOT}"

cd "$ROOT"
cargo run --locked -p hopspot-flash -- flash t-echo --local-build --yes --mount "$VOLUME"

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
wasm="$repo_root/target/wasm32-unknown-unknown/release/prns_nrf_dfu_wasm.wasm"
binding_stage="$(mktemp -d "${TMPDIR:-/tmp}/prns-nrf-dfu-node.XXXXXX")"
trap 'rm -rf -- "$binding_stage"' EXIT

if [[ "$(wasm-bindgen --version)" != "wasm-bindgen 0.2.126" ]]; then
    echo "wasm-bindgen 0.2.126 is required" >&2
    exit 2
fi

cargo test \
    --manifest-path "$repo_root/Cargo.toml" \
    --locked \
    --package prns-nrf-dfu-wasm
bash "$repo_root/tools/build/stage-web-flasher-nrf-dfu-wasm.sh" \
    "$binding_stage/web"
wasm-bindgen "$wasm" \
    --target nodejs \
    --out-dir "$binding_stage" \
    --out-name prns_nrf_dfu_core
node "$repo_root/prns-nrf-dfu-wasm/test/smoke.cjs" \
    "$binding_stage/prns_nrf_dfu_core.js"

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
wasm_dir="$repo_root/personal-rns-wasm"
public_dir="$repo_root/docs/website/public/browser-node-playground-console"

npm --prefix "$wasm_dir" run build:browser

mkdir -p "$public_dir/dist/smoke" "$public_dir/dist/ts" "$public_dir/pkg"
cp "$wasm_dir/smoke/index.html" "$public_dir/index.html"
cp "$wasm_dir/smoke/dist/smoke/smoke.js" "$public_dir/dist/smoke/smoke.js"
cp "$wasm_dir/smoke/dist/ts/index.js" "$public_dir/dist/ts/index.js"
cp "$wasm_dir/smoke/pkg/personal_rns_wasm.js" "$public_dir/pkg/personal_rns_wasm.js"
cp "$wasm_dir/smoke/pkg/personal_rns_wasm_bg.wasm" "$public_dir/pkg/personal_rns_wasm_bg.wasm"

perl -0pi -e 's#from "/pkg/personal_rns_wasm\.js"#from "../../pkg/personal_rns_wasm.js"#' \
    "$public_dir/dist/smoke/smoke.js"

echo "staged browser node playground console at $public_dir"

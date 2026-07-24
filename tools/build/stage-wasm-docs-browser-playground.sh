#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
wasm_dir="$repo_root/prns-wasm"
example_dir="$wasm_dir/examples/browser-playground"
build_dir="$wasm_dir/target/browser-playground"
public_dir="$repo_root/docs/website/public/browser-node-playground-console"

native_path() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -w "$1"
    else
        printf '%s' "$1"
    fi
}

home_native="$(native_path "$HOME")"
cargo_native="$(native_path "${CARGO_HOME:-$HOME/.cargo}")"
rustup_native="$(native_path "${RUSTUP_HOME:-$HOME/.rustup}")"
repo_native="$(native_path "$repo_root")"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$home_native=~ --remap-path-prefix=$cargo_native=/cargo --remap-path-prefix=$rustup_native=/rustc --remap-path-prefix=$repo_native=/prns"

npm --prefix "$wasm_dir" run build:playground

mkdir -p "$public_dir/sdk" "$public_dir/pkg"
cp "$example_dir/index.html" "$public_dir/index.html"
cp "$example_dir/styles.css" "$public_dir/styles.css"
cp "$build_dir/examples/browser-playground/lxmf.js" "$public_dir/lxmf.js"
cp "$build_dir/examples/browser-playground/main.js" "$public_dir/main.js"
cp "$build_dir/examples/browser-playground/outcomes.js" "$public_dir/outcomes.js"
cp "$build_dir/examples/browser-playground/presentation.js" "$public_dir/presentation.js"
cp "$build_dir/examples/browser-playground/state.js" "$public_dir/state.js"
cp "$build_dir/examples/browser-playground/view.js" "$public_dir/view.js"
cp "$build_dir/ts/auto_wifi.js" "$public_dir/sdk/auto_wifi.js"
cp "$build_dir/ts/casework.js" "$public_dir/sdk/casework.js"
cp "$build_dir/ts/index.js" "$public_dir/sdk/index.js"
cp "$build_dir/pkg/prns_wasm.js" "$public_dir/pkg/prns_wasm.js"
cp "$build_dir/pkg/prns_wasm_bg.wasm" "$public_dir/pkg/prns_wasm_bg.wasm"

echo "staged the browser transport playground at $public_dir"

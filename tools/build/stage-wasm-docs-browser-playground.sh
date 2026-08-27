#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
wasm_dir="$repo_root/prns-wasm"
example_dir="$wasm_dir/examples/browser-playground"
build_dir="$wasm_dir/target/browser-playground"
public_dir="${1:-$repo_root/docs/website/public/browser-node-playground-console}"

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

if [[ -n "${PRNS_SOURCE_ARCHIVE:-}" ]]; then
    (
        cd "$wasm_dir"
        cargo build --locked --release --target wasm32-unknown-unknown --features source-archive
        wasm-bindgen target/wasm32-unknown-unknown/release/prns_wasm.wasm \
            --target web \
            --out-dir target/browser-playground/pkg
        npm run build:playground:ts
    )
else
    npm --prefix "$wasm_dir" run build:playground
fi

if [[ -e "$public_dir/sdk" ]]; then
    rm -rf -- "$public_dir/sdk"
fi
mkdir -p "$public_dir/sdk" "$public_dir/pkg"
cp "$example_dir/index.html" "$public_dir/index.html"
cp "$example_dir/styles.css" "$public_dir/styles.css"
cp "$build_dir/prns-wasm/examples/browser-playground/bluetooth.js" "$public_dir/bluetooth.js"
cp "$build_dir/prns-wasm/examples/browser-playground/lxmf.js" "$public_dir/lxmf.js"
cp "$build_dir/prns-wasm/examples/browser-playground/main.js" "$public_dir/main.js"
cp "$build_dir/prns-wasm/examples/browser-playground/outcomes.js" "$public_dir/outcomes.js"
cp "$build_dir/prns-wasm/examples/browser-playground/presentation.js" "$public_dir/presentation.js"
cp "$build_dir/prns-wasm/examples/browser-playground/state.js" "$public_dir/state.js"
cp "$build_dir/prns-wasm/examples/browser-playground/view.js" "$public_dir/view.js"
cp -R "$build_dir/prns-js/src/." "$public_dir/sdk/"
cp "$example_dir/sdk/index.js" "$public_dir/sdk/index.js"
cp "$example_dir/sdk/package.json" "$public_dir/sdk/package.json"
cp "$build_dir/pkg/prns_wasm.js" "$public_dir/pkg/prns_wasm.js"
cp "$build_dir/pkg/prns_wasm_bg.wasm" "$public_dir/pkg/prns_wasm_bg.wasm"

sdk_entry_native="$(native_path "$public_dir/sdk/index.js")"
node --input-type=module \
    -e 'const { pathToFileURL } = await import("node:url"); await import(pathToFileURL(process.argv[1]).href)' \
    "$sdk_entry_native"

echo "staged the browser transport playground at $public_dir"

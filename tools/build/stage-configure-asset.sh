#!/usr/bin/env bash
# Build the headless-config webUI hosted asset (`/assets/configure/`) and stage
# it into the Dioxus website's public tree. The `/configure` page loads
# `/assets/configure/configure.js` via `document::eval`; that entry drives the
# WebUSB config lane through the prns-wasm codec + the prns-js browser session.
#
# Layout staged under <public>/assets/configure/:
#   configure.js                      tsc-emitted entry (WebUSB + bridge)
#   sdk/index.js                      re-export shim
#   sdk/browser/**.js                 tsc-emitted prns-js browser modules
#   sdk/{casework,contract,...}.js    tsc-emitted prns-js shared modules
#   pkg/prns_wasm.js                  wasm-bindgen --target web glue
#   pkg/prns_wasm_bg.wasm             compiled wasm
#
# Mirrors tools/build/stage-wasm-docs-browser-playground.sh (same wasm-bindgen
# + tsc-emit pipeline; the config webUI needs the codec, so unlike the pure-JS
# web flasher it cannot be a single esbuild bundle).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
wasm_dir="$repo_root/prns-wasm"
build_dir="$wasm_dir/target/configure"
public_dir="${1:-$repo_root/docs/website/public/assets/configure}"

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
            --out-dir target/configure/pkg
        npm run build:configure:ts
    )
else
    npm --prefix "$wasm_dir" run build:configure
fi

mkdir -p "$public_dir/sdk/browser" "$public_dir/pkg"

# Entry + sdk shim.
cp "$build_dir/prns-wasm/examples/configure/configure.js" "$public_dir/configure.js"
cp "$wasm_dir/examples/configure/sdk/index.js" "$public_dir/sdk/index.js"

# prns-js browser modules (whole emitted browser/ tree, incl. auto_wifi/).
cp -r "$build_dir/prns-js/src/browser/." "$public_dir/sdk/browser/"

# prns-js shared modules imported by the browser entry via ../
cp "$build_dir/prns-js/src/async_lanes.js" "$public_dir/sdk/async_lanes.js"
cp "$build_dir/prns-js/src/casework.js" "$public_dir/sdk/casework.js"
cp "$build_dir/prns-js/src/contract.generated.js" "$public_dir/sdk/contract.generated.js"
cp "$build_dir/prns-js/src/contract.js" "$public_dir/sdk/contract.js"
cp "$build_dir/prns-js/src/memory_resource.js" "$public_dir/sdk/memory_resource.js"

# wasm-bindgen glue + binary.
cp "$build_dir/pkg/prns_wasm.js" "$public_dir/pkg/prns_wasm.js"
cp "$build_dir/pkg/prns_wasm_bg.wasm" "$public_dir/pkg/prns_wasm_bg.wasm"

echo "staged the headless config webUI at $public_dir"
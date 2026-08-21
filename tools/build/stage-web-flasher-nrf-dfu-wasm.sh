#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
output="${1:-$repo_root/docs/website/target/hosted-assets/nrf-dfu}"
wasm="$repo_root/target/wasm32-unknown-unknown/release/prns_nrf_dfu_wasm.wasm"
binding_stage="$(mktemp -d "${TMPDIR:-/tmp}/prns-nrf-dfu-wasm.XXXXXX")"
trap 'rm -rf -- "$binding_stage"' EXIT

if [[ "$(wasm-bindgen --version)" != "wasm-bindgen 0.2.126" ]]; then
    echo "wasm-bindgen 0.2.126 is required" >&2
    exit 2
fi

repo_native="$repo_root"
rust_sysroot="$(rustc --print sysroot)"
user_home="${HOME:?}"
cargo_home="${CARGO_HOME:-$user_home/.cargo}"
rust_sysroot_path="$rust_sysroot"
user_home_path="$user_home"
cargo_home_path="$cargo_home"
if command -v cygpath >/dev/null 2>&1; then
    repo_native="$(cygpath -w "$repo_root")"
    rust_sysroot="$(cygpath -w "$rust_sysroot")"
    user_home="$(cygpath -w "$user_home")"
    cargo_home="$(cygpath -w "$cargo_home")"
fi
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$user_home=/build --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$rust_sysroot=/rust --remap-path-prefix=$repo_native=/prns"

cargo build \
    --manifest-path "$repo_root/Cargo.toml" \
    --locked \
    --package prns-nrf-dfu-wasm \
    --release \
    --target wasm32-unknown-unknown
wasm-bindgen "$wasm" \
    --target web \
    --out-dir "$binding_stage" \
    --out-name prns_nrf_dfu_core
for private_path in \
    "$repo_root" \
    "$repo_native" \
    "$rust_sysroot_path" \
    "$rust_sysroot" \
    "$cargo_home_path" \
    "$cargo_home" \
    "$user_home_path" \
    "$user_home"; do
    if grep -a -F "$private_path" "$wasm" "$binding_stage"/* >/dev/null; then
        echo "Nordic DFU browser core retained a private build path" >&2
        exit 1
    fi
done

artifacts=(
    prns_nrf_dfu_core.js
    prns_nrf_dfu_core.d.ts
    prns_nrf_dfu_core_bg.wasm
    prns_nrf_dfu_core_bg.wasm.d.ts
)
if [[ -e "$output" && ( ! -d "$output" || -L "$output" ) ]]; then
    echo "Nordic DFU browser core output must be a real directory: $output" >&2
    exit 2
fi
mkdir -p "$output"
if find "$output" -mindepth 1 -maxdepth 1 \
    ! \( -name 'prns_nrf_dfu_core.js' \
        -o -name 'prns_nrf_dfu_core.d.ts' \
        -o -name 'prns_nrf_dfu_core_bg.wasm' \
        -o -name 'prns_nrf_dfu_core_bg.wasm.d.ts' \) \
    -print -quit | grep -q .; then
    echo "Nordic DFU browser core output contains an unowned artifact: $output" >&2
    exit 2
fi
for artifact in "${artifacts[@]}"; do
    if [[ -e "$output/$artifact" && ( ! -f "$output/$artifact" || -L "$output/$artifact" ) ]]; then
        echo "Nordic DFU browser core artifact is not a regular file: $output/$artifact" >&2
        exit 2
    fi
    test -s "$binding_stage/$artifact"
    cp "$binding_stage/$artifact" "$output/$artifact"
done

echo "staged the Nordic DFU browser core at $output"

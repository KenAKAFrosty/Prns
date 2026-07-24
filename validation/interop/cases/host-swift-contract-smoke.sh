#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch="$(mktemp -d /tmp/prns-host-swift.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked
python3 tools/release/package-host-native.py \
    --target x86_64-unknown-linux-gnu \
    --library prns-host/abi/c/target/debug/libprns_host.so \
    --library prns-host/abi/c/target/debug/libprns_host.a \
    --output "$scratch/native"
env \
    PKG_CONFIG_PATH="$scratch/native/lib/pkgconfig" \
    LD_LIBRARY_PATH="$scratch/native/lib" \
    CLANG_MODULE_CACHE_PATH="$scratch/clang-cache" \
    XDG_CONFIG_HOME="$scratch/config" \
    XDG_CACHE_HOME="$scratch/cache" \
    swift test \
        --package-path prns-host/bindings/swift \
        --scratch-path "$scratch/build"

echo "HOST_SWIFT_CONTRACT_SMOKE_OK"

#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch="$(mktemp -d /tmp/prns-host-swift.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64)
        host_target="aarch64-apple-darwin"
        dynamic_library="libprns_host.dylib"
        ;;
    Darwin-x86_64)
        host_target="x86_64-apple-darwin"
        dynamic_library="libprns_host.dylib"
        ;;
    Linux-aarch64)
        host_target="aarch64-unknown-linux-gnu"
        dynamic_library="libprns_host.so"
        ;;
    Linux-x86_64)
        host_target="x86_64-unknown-linux-gnu"
        dynamic_library="libprns_host.so"
        ;;
    *)
        echo "unsupported Swift contract host: $(uname -s)-$(uname -m)" >&2
        exit 1
        ;;
esac

python3 tools/release/package-host-native.py \
    --target "$host_target" \
    --library "prns-host/abi/c/target/debug/$dynamic_library" \
    --library prns-host/abi/c/target/debug/libprns_host.a \
    --output "$scratch/native"
env \
    PKG_CONFIG_PATH="$scratch/native/lib/pkgconfig" \
    LD_LIBRARY_PATH="$scratch/native/lib" \
    DYLD_LIBRARY_PATH="$scratch/native/lib" \
    CLANG_MODULE_CACHE_PATH="$scratch/clang-cache" \
    XDG_CONFIG_HOME="$scratch/config" \
    XDG_CACHE_HOME="$scratch/cache" \
    swift test \
        --package-path prns-host/bindings/swift \
        --scratch-path "$scratch/build"

echo "HOST_SWIFT_CONTRACT_SMOKE_OK"

#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch="$(mktemp -d /tmp/prns-host-c.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked

loader_variable="LD_LIBRARY_PATH"
if [[ "$(uname -s)" == "Darwin" ]]; then
    loader_variable="DYLD_LIBRARY_PATH"
fi

for compiler in cc c++; do
    name="$(basename "$compiler")"
    standard="c11"
    if [[ "$name" == "c++" ]]; then
        standard="c++17"
    fi
    compile_args=("-std=$standard")
    if [[ "$name" == "c++" ]]; then
        compile_args+=(-x c++)
    fi
    mkdir -p "$scratch/state-$name"
    "$compiler" \
        "${compile_args[@]}" \
        -Wall \
        -Wextra \
        -Werror \
        -Iprns-host/abi/c/include \
        prns-host/abi/c/tests/persistent-two-node-smoke.c \
        -Lprns-host/abi/c/target/debug \
        -lprns_host \
        -lpthread \
        -ldl \
        -lm \
        -o "$scratch/journey-$name"
    env "${loader_variable}=${root}/prns-host/abi/c/target/debug" \
        "$scratch/journey-$name" \
        prns-host/conformance/persistent-two-node-v1.json \
        prns-host/conformance/interface-configs-v1.json \
        "$scratch/state-$name" \
        "$(cat VERSION)"
done

echo "HOST_C_CONTRACT_SMOKE_OK"

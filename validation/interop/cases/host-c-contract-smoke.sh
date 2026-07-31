#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch="$(mktemp -d /tmp/prns-host-c.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked

for compiler in cc c++; do
    name="$(basename "$compiler")"
    standard="c11"
    if [[ "$name" == "c++" ]]; then
        standard="c++17"
    fi
    mkdir -p "$scratch/state-$name"
    "$compiler" \
        "-std=$standard" \
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
    env LD_LIBRARY_PATH="${root}/prns-host/abi/c/target/debug" \
        "$scratch/journey-$name" \
        prns-host/conformance/persistent-two-node-v2.json \
        "$scratch/state-$name"
done

echo "HOST_C_CONTRACT_SMOKE_OK"

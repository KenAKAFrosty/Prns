#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch="$(mktemp -d /tmp/prns-host-julia.XXXXXX)"
trap 'rm -rf "$scratch"' EXIT
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked
for threads in 1 2; do
    env \
        PRNS_HOST_LIBRARY="${root}/prns-host/abi/c/target/debug/libprns_host.so" \
        JULIA_DEPOT_PATH="$scratch/depot" \
        julia \
            --project=prns-host/bindings/julia \
            --threads="$threads" \
            -e 'using PersonalRns; include("prns-host/bindings/julia/test/runtests.jl")'
done

echo "HOST_JULIA_CONTRACT_SMOKE_OK"

#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"

cargo build --manifest-path prns-host/abi/c/Cargo.toml --locked
env \
    PYTHONPATH="${root}/prns-host/bindings/python/src" \
    PRNS_HOST_LIBRARY="${root}/prns-host/abi/c/target/debug/libprns_host.so" \
    python3 prns-host/bindings/python/tests/smoke.py

echo "HOST_PYTHON_CONTRACT_SMOKE_OK"

#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo clippy -p prns-core --features external-alloc --all-targets --locked -- -D warnings
cargo test -p prns-core --features external-alloc --locked

echo "EXTERNAL_ALLOC_GATE_OK"

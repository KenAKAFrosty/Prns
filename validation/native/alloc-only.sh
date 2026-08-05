#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

# The shape the embedded boards actually ship: no std, alloc plus the caller-chosen heap region.
# external-alloc.sh keeps the default features, so `std` stays on there and this configuration is
# never compiled by any registered suite.
cargo clippy -p prns-core --no-default-features --features alloc,external-alloc --all-targets --locked -- -D warnings
cargo test -p prns-core --no-default-features --features alloc,external-alloc --locked

echo "ALLOC_ONLY_GATE_OK"

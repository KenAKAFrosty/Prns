#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo clippy --manifest-path prns-ffi/Cargo.toml --all-targets --locked -- -D warnings
cargo clippy --manifest-path personal-hopspot/desktop/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path personal-hopspot/desktop/Cargo.toml --locked

echo "WINDOWS_DESKTOP_GATE_OK"

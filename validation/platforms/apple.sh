#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo clippy --manifest-path prns-ffi/Cargo.toml --all-features --all-targets --locked -- -D warnings
cargo clippy --manifest-path prns-interfaces/impls/tokio/Cargo.toml --all-features --all-targets --locked -- -D warnings
cargo clippy --manifest-path personal-hopspot/mobile/ios/rust/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path personal-hopspot/mobile/ios/rust/Cargo.toml --release --locked
cargo clippy --manifest-path personal-hopspot/desktop/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path personal-hopspot/desktop/Cargo.toml --locked

echo "APPLE_PLATFORMS_GATE_OK"

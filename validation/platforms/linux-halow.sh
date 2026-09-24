#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo test --locked --manifest-path prns-ffi/Cargo.toml --features linux-packet --lib ethernet
cargo clippy --locked --manifest-path prns-ffi/Cargo.toml --features linux-packet --all-targets -- -D warnings
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features wifi-halow --lib wifi_halow
cargo clippy --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features wifi-halow --all-targets -- -D warnings

cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml --features wifi-halow,websocket,wifi-auto --all-targets
cargo clippy --locked --manifest-path personal-hopspot/headless/Cargo.toml --features wifi-halow,websocket,wifi-auto --all-targets -- -D warnings

#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

cargo clippy --manifest-path prns-config/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path prns-config/Cargo.toml --locked
cargo clippy --manifest-path prns-host/abi/c/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path prns-host/abi/c/Cargo.toml --locked
cc -std=c11 -Wall -Wextra -Werror -fsyntax-only prns-host/abi/c/tests/header-smoke.c
python3 validation/run.py run --suite host-c-contract
cargo clippy --manifest-path prnsd/Cargo.toml --workspace --all-features --all-targets --locked -- -D warnings
cargo test --manifest-path prnsd/Cargo.toml --workspace --all-features --locked
cargo clippy --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --all-targets --locked -- -D warnings
cargo test --manifest-path prns-runtime/impls/tokio/Cargo.toml --all-features --locked
cargo clippy --manifest-path prns-runtime/impls/embassy/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path prns-runtime/impls/embassy/Cargo.toml --locked
cargo clippy --manifest-path prns-interfaces/impls/tokio/Cargo.toml --all-features --all-targets --locked -- -D warnings
cargo test --manifest-path prns-interfaces/impls/tokio/Cargo.toml --all-features --locked
cargo clippy --manifest-path prns-interfaces/impls/embassy/Cargo.toml --all-features --all-targets --locked -- -D warnings
cargo clippy --manifest-path personal-hopspot/desktop/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path personal-hopspot/desktop/Cargo.toml --locked
cargo clippy --manifest-path prns-wasm/Cargo.toml --target wasm32-unknown-unknown --all-targets --locked -- -D warnings

echo "HOST_WORKSPACES_GATE_OK"

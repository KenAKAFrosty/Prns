#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
mode="${1:---entrypoints}"
if [[ "$mode" == --* ]]; then
    shift
else
    mode="--entrypoints"
fi

geiger_version="$(cargo geiger --version 2>/dev/null || true)"
if [[ "$geiger_version" != "cargo-geiger 0.13.0" ]]; then
    echo "cargo-geiger 0.13.0 is required: cargo install cargo-geiger --version 0.13.0 --locked" >&2
    exit 2
fi
geiger_options=(--quiet --color never --locked)

entrypoints() {
    echo "[geiger:entrypoint] personal-rns"
    cargo geiger "${geiger_options[@]}" --forbid-only --manifest-path "$root/personal-rns/Cargo.toml" "$@"
    echo "[geiger:entrypoint] Tokio interfaces"
    cargo geiger "${geiger_options[@]}" --forbid-only --all-features --manifest-path "$root/prns-interfaces/impls/tokio/Cargo.toml" "$@"
    echo "[geiger:entrypoint] platform FFI quarantine"
    cargo geiger "${geiger_options[@]}" --forbid-only --manifest-path "$root/prns-ffi/Cargo.toml" "$@"
}

inventory() {
    local status=0
    echo "[geiger:inventory] personal-rns default dependency graph"
    cargo geiger "${geiger_options[@]}" --include-tests --manifest-path "$root/personal-rns/Cargo.toml" "$@" || status=$?
    echo "[geiger:inventory] Tokio interface dependency graph"
    cargo geiger "${geiger_options[@]}" --include-tests --all-features --manifest-path "$root/prns-interfaces/impls/tokio/Cargo.toml" "$@" || status=$?
    echo "[geiger:inventory] platform FFI dependency graph"
    cargo geiger "${geiger_options[@]}" --include-tests --manifest-path "$root/prns-ffi/Cargo.toml" "$@" || status=$?
    return "$status"
}

case "$mode" in
    --entrypoints)
        entrypoints "$@"
        ;;
    --inventory)
        inventory "$@"
        ;;
    --all)
        entrypoints "$@"
        inventory "$@"
        ;;
    *)
        echo "usage: validation/hardening/geiger.sh [--entrypoints|--inventory|--all] [cargo-geiger options]" >&2
        exit 2
        ;;
esac

echo "GEIGER_SCAN_COMPLETE"

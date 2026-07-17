#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="x86_64-unknown-linux-gnu"
requested="${1:-all}"

if (($# > 1)); then
    echo "usage: scripts/sanitizers.sh [address|leak|thread|all]" >&2
    exit 2
fi

if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
    echo "sanitizers.sh supports Linux x86_64; found $(uname -s) $(uname -m)" >&2
    exit 2
fi

case "$requested" in
    address|leak|thread)
        sanitizers=("$requested")
        ;;
    all)
        sanitizers=(address leak thread)
        ;;
    *)
        echo "usage: scripts/sanitizers.sh [address|leak|thread|all]" >&2
        exit 2
        ;;
esac

if ! rustup run nightly rustc --version >/dev/null 2>&1; then
    rustup toolchain install nightly --profile minimal
fi
rustup component add --toolchain nightly rust-src llvm-tools-preview

run_suite() {
    local sanitizer="$1"
    local options_name="$2"
    local options_value="$3"
    local label="$4"
    local manifest="$5"
    shift 5

    echo "[$sanitizer] $label"
    env \
        CARGO_TARGET_DIR="$root/target/hardening/$sanitizer" \
        RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Zsanitizer=$sanitizer -Copt-level=2 -Cdebuginfo=1" \
        "$options_name=$options_value" \
        cargo +nightly test \
            --quiet \
            --locked \
            -Zbuild-std \
            --target "$target" \
            --manifest-path "$manifest" \
            --lib \
            --tests \
            "$@" \
            -- \
            --test-threads=1
}

for sanitizer in "${sanitizers[@]}"; do
    case "$sanitizer" in
        address)
            options_name="ASAN_OPTIONS"
            options_value="detect_leaks=0:halt_on_error=1"
            ;;
        leak)
            options_name="LSAN_OPTIONS"
            options_value="exitcode=23:report_objects=1"
            ;;
        thread)
            options_name="TSAN_OPTIONS"
            options_value="halt_on_error=1:print_suppressions=1:suppressions=$root/scripts/tsan-suppressions.txt"
            ;;
    esac

    run_suite "$sanitizer" "$options_name" "$options_value" \
        "prns-core" "$root/Cargo.toml" -p prns-core
    run_suite "$sanitizer" "$options_name" "$options_value" \
        "prns-runtime-tokio" "$root/prns-runtime/impls/tokio/Cargo.toml"
    run_suite "$sanitizer" "$options_name" "$options_value" \
        "cross-crate integration" "$root/prns-integration-tests/Cargo.toml"
    run_suite "$sanitizer" "$options_name" "$options_value" \
        "Tokio interfaces" "$root/prns-interfaces/tokio/Cargo.toml" --all-features
done

echo "SANITIZER_GATE_OK"

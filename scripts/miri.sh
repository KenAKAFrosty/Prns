#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if ! rustup run nightly rustc --version >/dev/null 2>&1; then
    rustup toolchain install nightly --profile minimal
fi
rustup component add --toolchain nightly miri rust-src
cargo +nightly miri setup

export PROPTEST_CASES="${PROPTEST_CASES:-32}"
export PROPTEST_DISABLE_FAILURE_PERSISTENCE="${PROPTEST_DISABLE_FAILURE_PERSISTENCE:-1}"

base_flags="${MIRIFLAGS:-} -Zmiri-env-forward=PROPTEST_CASES -Zmiri-env-forward=PROPTEST_DISABLE_FAILURE_PERSISTENCE"

run_miri() {
    local model="$1"
    shift
    local flags="$base_flags"
    if [[ "$model" == "tree" ]]; then
        flags="$flags -Zmiri-tree-borrows"
    fi
    echo "[miri:$model] ${*:-all prns-core tests}"
    MIRIFLAGS="$flags" cargo +nightly miri test --locked -p prns-core -- "$@" --test-threads=1
}

mode="${1:---quick}"
case "$mode" in
    --quick)
        shift || true
        if (($#)); then
            echo "usage: scripts/miri.sh [--quick|--full|--stacked [FILTER...]|--tree [FILTER...]]" >&2
            exit 2
        fi
        for filter in \
            wire::tests \
            routing::routes::impls::fixed_array::tests \
            routing::routes::impls::fixed_indexed::tests \
            routing::links::resources::table::core::tests \
            interfaces::bluetooth_auto::core::tests
        do
            run_miri stacked "$filter"
        done
        for filter in \
            crypto::token::stream_tests \
            identity::in_memory::tests \
            routing::links::resources::streamed_open::tests
        do
            run_miri tree "$filter"
        done
        ;;
    --full)
        shift
        run_miri tree "$@"
        ;;
    --stacked)
        shift
        run_miri stacked "$@"
        ;;
    --tree)
        shift
        run_miri tree "$@"
        ;;
    *)
        run_miri tree "$@"
        ;;
esac

echo "MIRI_GATE_OK"

#!/usr/bin/env bash
# Host-run tests for the embassy seam and its workers: the grant ring's lease
# discipline (runtime semantics plus compile_fail doctests on the lease type)
# and the esp-now worker's drain property against a recording mock link. The
# ESP cross-build gate keeps these compiling for the target; this gate keeps
# their behavior honest on every step.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/1] embassy seam + workers: host test run (std + embassy-host)"
cargo test -p personal-rns --features embassy-host

echo "EMBASSY_HOST_TESTS_OK"

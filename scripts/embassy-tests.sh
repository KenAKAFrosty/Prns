#!/usr/bin/env bash
# Host-run tests for the embassy seam + reactor: the grant ring's lease discipline
# (runtime semantics plus compile_fail doctests on the lease type) and the reactor's
# embassy driver. The ESP cross-build gate keeps these compiling for the target; this
# gate keeps their behavior honest on every step.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/1] embassy seam + reactor: host test run (std + embassy-host)"
cargo test -p prns-runtime --features embassy-host

echo "EMBASSY_TESTS_OK"

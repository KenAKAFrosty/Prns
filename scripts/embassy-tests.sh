#!/usr/bin/env bash
# Host-run tests for the embassy seam + reactor: the grant ring's lease discipline
# (runtime semantics plus compile_fail doctests on the lease type) and the reactor's
# embassy driver. The ESP cross-build gate keeps these compiling for the target; this
# gate keeps their behavior honest on every step.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/1] embassy seam + reactor: host test run (std + embassy-host)"
cargo test --manifest-path prns-runtime/impls/embassy/Cargo.toml -- \
  --skip a_recipe_node_hears_an_ifac_announce_a_supervisor_stands_a_peer_up_for

echo "EMBASSY_TESTS_OK"

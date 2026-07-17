#!/usr/bin/env bash
# Host-run tests for the tokio seam and host: the grant ring's lease discipline
# (runtime semantics, compile_fail doctests, the cancellation-keeps-the-packet
# property) and the cross-task wake paths under a real multi-thread executor.
# The live-socket capstones ride the prns-integration-tests workspace.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/2] tokio seam + host: host test run"
cargo test --manifest-path prns-runtime/impls/tokio/Cargo.toml

echo "[2/2] integration capstones (engine + interface impls, public API)"
(cd prns-integration-tests && cargo test)

echo "TOKIO_HOST_TESTS_OK"

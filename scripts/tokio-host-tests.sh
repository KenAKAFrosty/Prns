#!/usr/bin/env bash
# Host-run tests for the tokio seam and host: the grant ring's lease discipline
# (runtime semantics, compile_fail doctests, the cancellation-keeps-the-packet
# property) and the cross-task wake paths under a real multi-thread executor.
# The tcp face rides along: the TCP interfaces and their live-socket capstone.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/1] tokio seam + host: host test run (std + tokio-host + tcp)"
cargo test -p personal-rns --features tokio-host,tcp

echo "TOKIO_HOST_TESTS_OK"

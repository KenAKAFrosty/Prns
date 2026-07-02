#!/usr/bin/env bash
# Source-based line/region coverage via cargo-llvm-cov (accurate, LLVM-instrumented,
# no nightly required).
#   Install: rustup component add llvm-tools-preview && cargo install cargo-llvm-cov
#
# The engine number is the one that matters most — prns-core owns the wire and
# behaviour contract, so a coverage gate, when we add one, should read its summary.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[coverage] prns-core (summary)"
cargo llvm-cov -p prns-core --summary-only

echo "[coverage] workspace (HTML -> target/llvm-cov/html/index.html)"
cargo llvm-cov --workspace --html

echo "COVERAGE_REPORT_OK"

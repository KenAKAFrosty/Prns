#!/usr/bin/env bash
# Build and run the curated, three-sample release matrix.
#
# Full Apple energy run:
#   ./build.sh
#   sudo env "PATH=$PATH" ./run-release-matrix.sh
#
# Inspect or smoke the matrix without publishing:
#   ./run-release-matrix.sh --dry-run
#   ./run-release-matrix.sh --smoke
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
RUNNER="$HERE/target/release/benchmark_runner"
source "$HERE/host-rustflags.sh"
append_benchmark_host_rustflags

if [ "$(id -u)" -ne 0 ]; then
  cargo build --release --quiet --manifest-path "$HERE/Cargo.toml" --bins
fi
if [ ! -x "$RUNNER" ]; then
  echo "benchmark_runner is not built; run ./build.sh as your normal user first" >&2
  exit 1
fi

exec "$RUNNER" suite release --samples 3 "$@"

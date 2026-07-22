#!/usr/bin/env bash
# Build the two-part release harness: Prns and the compiled RNS 1.4.0 reference.
# Run WITHOUT sudo so Cargo and Python caches stay user-owned; then measure with
#   sudo env "PATH=$PATH" ./run-release-matrix.sh
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"

HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/host-rustflags.sh"
append_benchmark_host_rustflags

echo "== Prns release benchmark harness =="
( cd "$HERE" && cargo build --quiet --release --bins )

echo "== Compiled RNS 1.4.0 reference =="
"$HERE/reference/prepare-compiled-reference.sh"

echo
echo "Built. Register this machine once with:  cargo run --release --bin describe_host"
echo "Then measure:  ./run-release-matrix.sh"
echo "Optional macOS energy:  sudo env \"PATH=\$PATH\" ./run-release-matrix.sh"

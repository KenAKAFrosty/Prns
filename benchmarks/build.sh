#!/usr/bin/env bash
# Build the unified benchmark harness: our own bins, plus every external port's interop node
# built against its pinned upstream (cloned into a gitignored external/<impl>/.upstream/). Run
# WITHOUT sudo so the cargo/go/crystal caches stay user-owned; then measure with
#   sudo env "PATH=$PATH" ./run.sh
set -euo pipefail
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"

HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/external/lib.sh"

echo "== Prns (orchestrator + scenario_node + renderer) =="
( cd "$HERE" && cargo build --quiet --release \
    --bin orchestrate --bin scenario_node --bin render_results --bin describe_host )

echo "== go-reticulum =="
clone_pinned "https://github.com/svanichkin/go-reticulum.git" 06621cc "$HERE/external/go-reticulum/.upstream"
( cd "$HERE/external/go-reticulum/interop" && go build -o go-node . )

echo
echo "Built. Register this machine once with:  cargo run --release --bin describe_host"
echo "Then measure:  sudo env \"PATH=\$PATH\" ./run.sh"

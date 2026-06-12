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

echo "== Leviculum =="
LEVICULUM="$HERE/external/leviculum/.upstream"
clone_pinned "https://codeberg.org/Lew_Palm/leviculum.git" 6f366ca "$LEVICULUM"
# The upstream's Local-IPC/RPC paths use Linux-only abstract Unix sockets unconditionally;
# cfg-gate them so the crate compiles on macOS (the TCP-only node never touches them). Applied
# idempotently — skip if it's already in the working tree.
LEVICULUM_PATCH="$HERE/external/leviculum/macos-portability.patch"
git -C "$LEVICULUM" apply --reverse --check "$LEVICULUM_PATCH" 2>/dev/null \
  || git -C "$LEVICULUM" apply "$LEVICULUM_PATCH"
( cd "$HERE/external/leviculum/interop" && cargo build --quiet --release \
    && cp target/release/leviculum-node leviculum-node )

echo
echo "Built. Register this machine once with:  cargo run --release --bin describe_host"
echo "Then measure:  sudo env \"PATH=\$PATH\" ./run.sh"

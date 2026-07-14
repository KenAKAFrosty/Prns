#!/usr/bin/env bash
# The one matrix run: every implementation through every scenario — throughput, conformance,
# and energy in a single pass over the unified orchestrator. Sustained scenarios run one node
# all-cores; interop scenarios run every initiator×responder pairing over localhost.
#
# Run under sudo for the energy axis (powermetrics on macOS, RAPL on Linux); without it,
# conformance + throughput still file and energy renders pending:
#   sudo env "PATH=$PATH" ./run.sh
#
# A short smoke pass (no energy needed) overrides every duration:
#   DURATION_MS=2000 ./run.sh
#
# Assumes the nodes are built (`./build.sh`); our own bins are (re)built here for convenience.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ORCH="$HERE/target/release/orchestrate"
source "$HERE/host-rustflags.sh"
append_benchmark_host_rustflags

# Build our own bins as the invoking user. Under sudo the binaries are already built —
# building as root would scribble root-owned artifacts into target/ (and rebuild deps under
# /var/root). So the energy run only measures; the build happens in your own shell.
if [ "$(id -u)" -ne 0 ]; then
  cargo build --release --quiet --manifest-path "$HERE/Cargo.toml" \
    --bin orchestrate --bin scenario_node --bin shaped_pipe
fi
if [ ! -x "$ORCH" ]; then
  echo "orchestrate is not built — run \`cargo build --release\` as your user (not root) first." >&2
  exit 1
fi

# The roster. Phase 2 extends these lists as each external port's interop node lands.
INTEROP_SCENARIOS=(single-firehose link-firehose-small-payload)
INTEROP_IMPLS=(self reference go-reticulum leviculum rns-cr lxmf-rs)
RESOURCE_SCENARIOS=(resource-transfer resource-bulk resource-bulk-compressed resource-bulk-compressible)
RESOURCE_IMPLS=(self reference)
CHANNEL_SCENARIOS=(channel-firehose-small-payload)
CHANNEL_IMPLS=(self reference)

# DURATION_MS overrides every scenario's wall-time for a quick smoke pass. Funnelled through
# one helper so an empty override never expands an empty array — macOS ships bash 3.2, where
# "${arr[@]}" on an empty array under `set -u` is itself an "unbound variable" error.
run_orch() {
  if [ -n "${DURATION_MS:-}" ]; then
    "$ORCH" "$@" --duration-ms "$DURATION_MS"
  else
    "$ORCH" "$@"
  fi
}

for scenario in "${INTEROP_SCENARIOS[@]}"; do
  for initiator in "${INTEROP_IMPLS[@]}"; do
    for responder in "${INTEROP_IMPLS[@]}"; do
      echo "== interop $scenario : $initiator -> $responder =="
      run_orch "$scenario" --initiator "$initiator" --responder "$responder" \
        || echo "  (failed: $scenario $initiator -> $responder)"
    done
  done
done

for scenario in "${RESOURCE_SCENARIOS[@]}"; do
  for initiator in "${RESOURCE_IMPLS[@]}"; do
    for responder in "${RESOURCE_IMPLS[@]}"; do
      echo "== resource $scenario : $initiator -> $responder =="
      run_orch "$scenario" --initiator "$initiator" --responder "$responder" \
        || echo "  (failed: $scenario $initiator -> $responder)"
    done
  done
done

for scenario in "${CHANNEL_SCENARIOS[@]}"; do
  for initiator in "${CHANNEL_IMPLS[@]}"; do
    for responder in "${CHANNEL_IMPLS[@]}"; do
      echo "== channel $scenario : $initiator -> $responder =="
      run_orch "$scenario" --initiator "$initiator" --responder "$responder" \
        || echo "  (failed: $scenario $initiator -> $responder)"
    done
  done
done

echo
echo "Matrix done. Render with:  cargo run --release --bin render_results"

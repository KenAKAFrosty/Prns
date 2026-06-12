#!/usr/bin/env bash
# One trunk run of the chain scenario: leaf -> five pure transport nodes -> leaf,
# six hops end to end. Usage:
#   ./run_chain.sh <initiator-impl> <trunk-impl> <responder-impl>   (impls: self | reference)
# Spawns the responder leaf, grows the trunk one node at a time (each chain node dials the
# previous READY address and reports its own), points the initiator at the trunk's head,
# and reports both leaves' RESULT lines plus each trunk node's CPU and peak RSS.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="${MANIFEST:-$HERE/scenarios/link-firehose-chain/manifest.json}"
TRUNK_LEN=5

INIT_IMPL="${1:?usage: run_chain.sh <initiator-impl> <trunk-impl> <responder-impl>}"
TRUNK_IMPL="${2:?trunk impl}"
RESP_IMPL="${3:?responder impl}"

node_cmd() {
  if [ "$1" = "self" ]; then
    echo "$HERE/target/release/scenario_node"
  else
    echo "python3 $HERE/reference/scenario_node.py"
  fi
}

PIN_CORES="${PIN_CORES:-$(cat /sys/devices/cpu_core/cpus 2>/dev/null || true)}"
PIN=""
if [ -n "$PIN_CORES" ] && command -v taskset >/dev/null 2>&1; then
  PIN="taskset -c $PIN_CORES"
fi

TMP="$(mktemp -d /tmp/chain-run.XXXXXX)"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null; done
}
trap cleanup EXIT

await_ready() {
  local log="$1"
  for _ in $(seq 1 60); do
    local found
    found="$(grep -h "READY" "$log" 2>/dev/null | sed -n 's/.*addr=//p' | head -1)"
    if [ -n "$found" ]; then
      echo "$found"
      return 0
    fi
    sleep 0.5
  done
  echo ""
}

$PIN $(node_cmd "$RESP_IMPL") "$MANIFEST" responder 127.0.0.1:0 > "$TMP/resp.log" 2>&1 &
PIDS+=($!)
UPSTREAM="$(await_ready "$TMP/resp.log")"
[ -n "$UPSTREAM" ] || { echo "CHAIN-FAIL responder never READY"; exit 1; }

TRUNK_PIDS=()
for i in $(seq 1 "$TRUNK_LEN"); do
  $PIN $(node_cmd "$TRUNK_IMPL") "$MANIFEST" chain "$UPSTREAM" > "$TMP/t$i.log" 2>&1 &
  PIDS+=($!)
  TRUNK_PIDS+=($!)
  UPSTREAM="$(await_ready "$TMP/t$i.log")"
  [ -n "$UPSTREAM" ] || { echo "CHAIN-FAIL trunk node $i never READY"; exit 1; }
done

INIT_OUT="$($PIN $(node_cmd "$INIT_IMPL") "$MANIFEST" initiator "$UPSTREAM" 2>/dev/null | grep RESULT)"
sleep 2
RESP_OUT="$(grep RESULT "$TMP/resp.log" || true)"

echo "CHAIN initiator=$INIT_IMPL trunk=${TRUNK_LEN}x$TRUNK_IMPL responder=$RESP_IMPL"
echo "CHAIN-INIT $INIT_OUT"
echo "CHAIN-RESP ${RESP_OUT:-no responder RESULT}"
for i in "${!TRUNK_PIDS[@]}"; do
  pid="${TRUNK_PIDS[$i]}"
  stats="$(ps -o cputime=,rss= -p "$pid" 2>/dev/null | awk '{print "cpu="$1" rss_kb="$2}')"
  echo "CHAIN-TRUNK node=$((i + 1)) ${stats:-gone}"
done

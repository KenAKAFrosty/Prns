#!/usr/bin/env bash
# One fan-in run: two initiators driving one responder directly. Usage:
#   ./run_fanin.sh <responder-impl> <sender1-impl> <sender2-impl>   (impls: self | reference)
# Sender2 "-" runs a single sender for a same-pinning 1:1 baseline; pair that with
#   MANIFEST=scenarios/request-response/manifest.json   (initiator_count 1)
# or the responder waits forever for a second link.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MANIFEST="${MANIFEST:-$HERE/scenarios/request-fanin/manifest.json}"

RESP_IMPL="${1:?usage: run_fanin.sh <responder-impl> <sender1-impl> <sender2-impl>}"
SEND1_IMPL="${2:?sender1 impl}"
SEND2_IMPL="${3:?sender2 impl (or - for a 1:1 baseline)}"

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

TMP="$(mktemp -d /tmp/fanin-run.XXXXXX)"
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
RESP_PID=$!
PIDS+=($!)
ADDR="$(await_ready "$TMP/resp.log")"
[ -n "$ADDR" ] || { echo "FANIN-FAIL responder never READY"; exit 1; }
ADDR1="${ADDR%%+*}"
ADDR2="${ADDR##*+}"

$PIN $(node_cmd "$SEND1_IMPL") "$MANIFEST" initiator "$ADDR1" > "$TMP/s1.log" 2>&1 &
S1_PID=$!
PIDS+=($!)
S2_PID=""
if [ "$SEND2_IMPL" != "-" ]; then
  $PIN $(node_cmd "$SEND2_IMPL") "$MANIFEST" initiator "$ADDR2" > "$TMP/s2.log" 2>&1 &
  S2_PID=$!
  PIDS+=($!)
fi

wait "$S1_PID"
[ -n "$S2_PID" ] && wait "$S2_PID"
sleep 2

echo "FANIN responder=$RESP_IMPL senders=$SEND1_IMPL+$SEND2_IMPL"
echo "FANIN-SEND1 $(grep RESULT "$TMP/s1.log" || echo "no RESULT")"
if [ -n "$S2_PID" ]; then
  echo "FANIN-SEND2 $(grep RESULT "$TMP/s2.log" || echo "no RESULT")"
fi
echo "FANIN-RESP $(grep RESULT "$TMP/resp.log" || echo "no RESULT")"

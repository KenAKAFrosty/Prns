#!/usr/bin/env bash
# One dakka run: N initiators driving one responder, the fan-in shape scaled up. Usage:
#   ./run_dakka.sh <responder-impl> <sender-impl> <sender-count>   (impls: self | reference)
# The responder is pinned to RESP_CORES (default 0-3, two physical P-cores) and the
# senders share SEND_CORES (default 8-15, the eight uniform E-cores) - the load-generator
# discipline web benches inherit from running wrk on a separate box: adding clients must
# never steal server CPU, or the curve measures the machine instead of the server.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC_MANIFEST="${MANIFEST:-$HERE/scenarios/request-fanin/manifest.json}"

RESP_IMPL="${1:?usage: run_dakka.sh <responder-impl> <sender-impl> <sender-count>}"
SEND_IMPL="${2:?sender impl}"
SEND_COUNT="${3:?sender count}"

node_cmd() {
  if [ "$1" = "self" ]; then
    echo "$HERE/target/release/scenario_node"
  else
    echo "python3 $HERE/reference/scenario_node.py"
  fi
}

RESP_CORES="${RESP_CORES:-0-3}"
SEND_CORES="${SEND_CORES:-8-15}"
PIN_RESP=""
PIN_SEND=""
if command -v taskset >/dev/null 2>&1; then
  PIN_RESP="taskset -c $RESP_CORES"
  PIN_SEND="taskset -c $SEND_CORES"
fi

TMP="$(mktemp -d /tmp/dakka-run.XXXXXX)"
python3 - "$SRC_MANIFEST" "$TMP/manifest.json" "$SEND_COUNT" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
count = int(sys.argv[3])
manifest["profile"]["initiator_count"] = count
manifest["roles"] = ["responder"] + ["initiator"] * count
json.dump(manifest, open(sys.argv[2], "w"))
PY
MANIFEST="$TMP/manifest.json"

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

$PIN_RESP $(node_cmd "$RESP_IMPL") "$MANIFEST" responder 127.0.0.1:0 > "$TMP/resp.log" 2>&1 &
RESP_PID=$!
PIDS+=($!)
(
  while [ -r "/proc/$RESP_PID/stat" ]; do
    cut -d' ' -f14,15 "/proc/$RESP_PID/stat" > "$TMP/resp_ticks" 2>/dev/null
    sleep 1
  done
) &
PIDS+=($!)
ADDR="$(await_ready "$TMP/resp.log")"
[ -n "$ADDR" ] || { echo "DAKKA-FAIL responder never READY"; exit 1; }
IFS='+' read -ra ADDRS <<< "$ADDR"

START_S=$SECONDS
SEND_PIDS=()
for i in $(seq 0 $((SEND_COUNT - 1))); do
  TARGET="${ADDRS[$((i % ${#ADDRS[@]}))]}"
  $PIN_SEND $(node_cmd "$SEND_IMPL") "$MANIFEST" initiator "$TARGET" > "$TMP/s$i.log" 2>&1 &
  SEND_PIDS+=($!)
  PIDS+=($!)
done

for pid in "${SEND_PIDS[@]}"; do wait "$pid"; done
WALL=$((SECONDS - START_S))
sleep 2
RESP_CPU_S="$(awk '{ printf "%.1f", ($1 + $2) / 100 }' "$TMP/resp_ticks" 2>/dev/null)"

echo "DAKKA responder=$RESP_IMPL senders=${SEND_IMPL}x${SEND_COUNT} wall_s=$WALL resp_cpu_s=${RESP_CPU_S:-unknown} resp_cores=$RESP_CORES send_cores=$SEND_CORES"
for i in $(seq 0 $((SEND_COUNT - 1))); do
  echo "DAKKA-SEND$i $(grep RESULT "$TMP/s$i.log" || echo "no RESULT")"
done
echo "DAKKA-RESP $(grep RESULT "$TMP/resp.log" || echo "no RESULT")"

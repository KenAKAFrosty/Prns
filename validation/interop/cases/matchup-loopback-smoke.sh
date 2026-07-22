#!/usr/bin/env bash
# Matchup loopback baseline: the reference-host / reference-client cell.
#
# Topology: a stock RNS shared instance (rnsd, the reference venv) is the transport host; two stock
# RNS apps (scenario_node.py in shared-instance mode) attach to it over the loopback bus and run the
# single-firehose scenario through it. No Prns code is involved — this is the all-reference baseline
# and oracle for the grid, and it proves the loopback topology plus the participant's shared-instance
# mode end to end before the Prns-host cells (which need the `local` feature) come online.
#
# Both ends are the pinned reference RNS 1.4.0 (benchmarks/reference/requirements.txt; $SMOKE_PYTHON
# if set, else the local reference venv). Prints PASS or FAIL and exits accordingly.
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
VENV_BIN="$ROOT/benchmarks/reference/.venv/bin"
VENV_PY="${SMOKE_PYTHON:-$VENV_BIN/python}"
RNSD="$VENV_BIN/rnsd"
NODE="$ROOT/benchmarks/reference/scenario_node.py"
MANIFEST="$ROOT/benchmarks/scenarios/single-firehose/manifest.json"
DURATION_MS="${DURATION_MS:-3000}"
RPC_KEY="$(printf '5a%.0s' $(seq 1 32))"

HOST_DIR="$(mktemp -d)"
HOST_LOG="$(mktemp)"
RESP_LOG="$(mktemp)"
INIT_LOG="$(mktemp)"
HOST_PID=""; RESP_PID=""; INIT_PID=""

cleanup() {
    for pid in "$INIT_PID" "$RESP_PID" "$HOST_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    rm -rf "$HOST_DIR"
}
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }
[ -x "$RNSD" ] || { echo "FAIL: rnsd not found at $RNSD"; exit 1; }
[ -f "$MANIFEST" ] || { echo "FAIL: manifest not found at $MANIFEST"; exit 1; }

PORT="$("$VENV_PY" - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); print(p)
PY
)"
[ -n "${PORT:-}" ] || { echo "FAIL: could not allocate a port"; exit 1; }
CTRL=$((PORT + 1))
echo "shared instance: bus=127.0.0.1:$PORT control=127.0.0.1:$CTRL"

cat > "$HOST_DIR/config" <<EOF
[reticulum]
  enable_transport = Yes
  share_instance = Yes
  shared_instance_type = tcp
  shared_instance_port = $PORT
  instance_control_port = $CTRL
  rpc_key = $RPC_KEY
  panic_on_interface_error = No

[logging]
  loglevel = 3
EOF

echo "starting the reference host (rnsd)..."
"$RNSD" --config "$HOST_DIR" > "$HOST_LOG" 2>&1 &
HOST_PID=$!

bus_up() {
    "$VENV_PY" - "$PORT" <<'PY'
import socket, sys
try:
    socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.3).close()
except OSError:
    sys.exit(1)
PY
}
for _ in $(seq 1 100); do bus_up && break; sleep 0.2; done
bus_up || { echo "FAIL: rnsd never bound the shared-instance bus"; tail -30 "$HOST_LOG"; exit 1; }
echo "host bus is up; starting the two app clients..."

export RNS_BENCH_SHARED_PORT="$PORT"
export RNS_BENCH_SHARED_RPC_KEY="$RPC_KEY"

"$VENV_PY" "$NODE" "$MANIFEST" responder shared "$DURATION_MS" > "$RESP_LOG" 2>/dev/null &
RESP_PID=$!
for _ in $(seq 1 100); do grep -q "READY" "$RESP_LOG" && break; sleep 0.2; done
grep -q "READY" "$RESP_LOG" || { echo "FAIL: responder never became READY"; tail -20 "$RESP_LOG"; exit 1; }

"$VENV_PY" "$NODE" "$MANIFEST" initiator shared "$DURATION_MS" > "$INIT_LOG" 2>/dev/null &
INIT_PID=$!

for _ in $(seq 1 200); do
    grep -q "RESULT" "$INIT_LOG" && grep -q "RESULT" "$RESP_LOG" && break
    sleep 0.25
done

INIT_RESULT="$(grep "RESULT" "$INIT_LOG" | head -1)"
RESP_RESULT="$(grep "RESULT" "$RESP_LOG" | head -1)"

field() { echo "$1" | tr ' ' '\n' | grep "^$2=" | cut -d= -f2; }

INIT_DELIVERED="$(field "$INIT_RESULT" delivered)"
INIT_SENT="$(field "$INIT_RESULT" sent)"
INIT_TIMEOUTS="$(field "$INIT_RESULT" timeouts)"
RESP_DELIVERED="$(field "$RESP_RESULT" delivered)"

echo "  initiator: $INIT_RESULT"
echo "  responder: $RESP_RESULT"

if [ -n "${INIT_DELIVERED:-}" ] && [ "${INIT_DELIVERED:-0}" -gt 0 ] \
   && [ -n "${RESP_DELIVERED:-}" ] && [ "${RESP_DELIVERED:-0}" -gt 0 ]; then
    echo "PASS: stock RNS apps ran single-firehose through a stock shared instance"
    echo "  sent=$INIT_SENT delivered=$INIT_DELIVERED timeouts=$INIT_TIMEOUTS (responder saw $RESP_DELIVERED)"
    exit 0
fi

echo "FAIL: no clean delivery through the shared instance"
echo "--- host log (tail) ---"; tail -30 "$HOST_LOG"
echo "--- responder log (tail) ---"; tail -20 "$RESP_LOG"
echo "--- initiator log (tail) ---"; tail -20 "$INIT_LOG"
exit 1

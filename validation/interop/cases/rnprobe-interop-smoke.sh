#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/validation/.venv/rns-rpc-1.4.0/bin/python}"
SERVER="$ROOT/validation/interop/peers/rns_rnprobe_server.py"
WORK="$(mktemp -d)"
CONFIG="$WORK/config"
SERVER_LOG="$WORK/server.log"
SERVER_PID=""

cleanup() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
    [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }

PORTS="$($PYTHON - <<'PY'
import socket

sockets = []
ports = []
for _ in range(2):
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    sockets.append(sock)
    ports.append(sock.getsockname()[1])
print(*ports)
for sock in sockets:
    sock.close()
PY
)" || { echo "FAIL: could not allocate loopback ports"; exit 1; }
set -- $PORTS
BUS_PORT="${1:-}"
CONTROL_PORT="${2:-}"
[ -n "$BUS_PORT" ] && [ -n "$CONTROL_PORT" ] || { echo "FAIL: empty shared-instance ports"; exit 1; }

"$PYTHON" "$SERVER" prepare "$CONFIG" "$BUS_PORT" "$CONTROL_PORT"
( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }
"$PYTHON" "$SERVER" serve "$CONFIG" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
    grep -q "RNPROBE_SERVER_READY" "$SERVER_LOG" && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNPROBE_SERVER_READY" "$SERVER_LOG" || {
    echo "FAIL: stock RNS rnprobe server never became ready"
    cat "$SERVER_LOG"
    exit 1
}

READY="$(sed -n 's/^RNPROBE_SERVER_READY //p' "$SERVER_LOG" | head -n 1)"
set -- $READY
PROBE_HASH="${1:-}"
SILENT_HASH="${2:-}"
[ -n "$PROBE_HASH" ] && [ -n "$SILENT_HASH" ] || { echo "FAIL: oracle destination hashes were unavailable"; exit 1; }

PROBE_RESULT="$($ROOT/prnsd/target/debug/prnsd probe --config "$CONFIG" -s 24 -n 2 -t 5 -w 0.1 -v rnstransport.probe "$PROBE_HASH" 2>&1)" || {
    echo "FAIL: Prnsd could not probe the stock RNS responder"
    echo "$PROBE_RESULT"
    cat "$SERVER_LOG"
    exit 1
}
VALID_REPLIES="$(printf '%s\n' "$PROBE_RESULT" | grep -c "Valid reply from <$PROBE_HASH>")"
[ "$VALID_REPLIES" -eq 2 ] && [[ "$PROBE_RESULT" == *"Sent 2, received 2, packet loss 0%"* ]] || {
    echo "FAIL: Prnsd did not settle two stock RNS delivery proofs"
    echo "$PROBE_RESULT"
    exit 1
}

SILENT_RESULT="$($ROOT/prnsd/target/debug/prnsd probe --config "$CONFIG" -n 1 -t 0.5 oracle.silent "$SILENT_HASH" 2>&1)"
SILENT_STATUS=$?
[ "$SILENT_STATUS" -eq 2 ] && [[ "$SILENT_RESULT" == *"Probe timed out"* ]] && [[ "$SILENT_RESULT" == *"Sent 1, received 0, packet loss 100%"* ]] || {
    echo "FAIL: Prnsd did not preserve stock rnprobe packet-loss semantics"
    echo "$SILENT_RESULT"
    echo "exit=$SILENT_STATUS"
    exit 1
}

echo "PASS: Prnsd probe exchanged delivery proofs with stock RNS 1.4.0 and preserved loss exit 2"

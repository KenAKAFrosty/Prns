#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/benchmarks/reference/.rpc-venv/bin/python}"
SERVER="$ROOT/prns-core/tests/interop/rns_rnstatus_server.py"
WORK="$(mktemp -d)"
CONFIG="$WORK/config"
MANAGEMENT_IDENTITY="$WORK/management_identity"
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
print(ports[0], ports[1])
for sock in sockets:
    sock.close()
PY
)" || { echo "FAIL: could not allocate loopback ports"; exit 1; }
set -- $PORTS
BUS_PORT="${1:-}"
CONTROL_PORT="${2:-}"
[ -n "$BUS_PORT" ] && [ -n "$CONTROL_PORT" ] || { echo "FAIL: empty shared-instance ports"; exit 1; }

"$PYTHON" "$SERVER" prepare "$CONFIG" "$BUS_PORT" "$CONTROL_PORT" "$MANAGEMENT_IDENTITY"
( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }
"$PYTHON" "$SERVER" serve "$CONFIG" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
    grep -q "RNSTATUS_SERVER_READY" "$SERVER_LOG" && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNSTATUS_SERVER_READY" "$SERVER_LOG" || {
    echo "FAIL: stock RNS rnstatus server never became ready"
    cat "$SERVER_LOG"
    exit 1
}

LOCAL_RESULT="$("$PRNSD" status --config "$CONFIG" --json 2>&1)" || {
    echo "FAIL: Prnsd could not query stock shared-instance RPC"
    echo "$LOCAL_RESULT"
    exit 1
}
echo "$LOCAL_RESULT" | "$PYTHON" -c 'import json, sys; report=json.load(sys.stdin); assert isinstance(report["interfaces"], list); assert report["transport_id"]' || {
    echo "FAIL: Prnsd did not decode stock local status"
    echo "$LOCAL_RESULT"
    exit 1
}

TRANSPORT_HASH="$($PYTHON "$SERVER" identity-hash "$CONFIG/storage/transport_identity")"
[ -n "$TRANSPORT_HASH" ] || { echo "FAIL: stock transport identity was unavailable"; exit 1; }
REMOTE_RESULT="$("$PRNSD" status --config "$CONFIG" -R "$TRANSPORT_HASH" -i "$MANAGEMENT_IDENTITY" -l -t 2>&1)" || {
    echo "FAIL: Prnsd could not query stock remote management"
    echo "$REMOTE_RESULT"
    exit 1
}
[[ "$REMOTE_RESULT" == *"Transport Instance <$TRANSPORT_HASH> running"* ]] || {
    echo "FAIL: Prnsd remote status did not identify the stock transport"
    echo "$REMOTE_RESULT"
    exit 1
}
[[ "$REMOTE_RESULT" == *"link table"* ]] || {
    echo "FAIL: Prnsd remote status did not decode the stock link count"
    echo "$REMOTE_RESULT"
    exit 1
}

echo "PASS: Prnsd status queried stock RNS 1.3.8 local RPC and authenticated remote management"

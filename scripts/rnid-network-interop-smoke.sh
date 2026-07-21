#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/benchmarks/reference/.rpc-venv/bin/python}"
SERVER="$ROOT/prns-core/tests/interop/rns_rnid_network_server.py"
WORK="$(mktemp -d)"
CONFIG="$WORK/config"
ANNOUNCE_IDENTITY="$WORK/announce.rid"
SERVER_LOG="$WORK/server.log"
SERVER_PID=""
BIN="$ROOT/prnsd/target/debug/prnsd"

cleanup() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
    [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null
    rm -rf -- "$WORK"
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

"$PYTHON" "$SERVER" prepare "$CONFIG" "$BUS_PORT" "$CONTROL_PORT" "$ANNOUNCE_IDENTITY"
( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }
"$PYTHON" "$SERVER" serve "$CONFIG" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
    grep -q "RNID_SERVER_READY" "$SERVER_LOG" && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNID_SERVER_READY" "$SERVER_LOG" || {
    echo "FAIL: stock RNS identity server never became ready"
    cat "$SERVER_LOG"
    exit 1
}

READY="$(sed -n 's/^RNID_SERVER_READY //p' "$SERVER_LOG" | head -n 1)"
set -- $READY
IDENTITY_HASH="${1:-}"
DESTINATION_HASH="${2:-}"
ANNOUNCE_HASH="${3:-}"
[ -n "$IDENTITY_HASH" ] && [ -n "$DESTINATION_HASH" ] && [ -n "$ANNOUNCE_HASH" ] || {
    echo "FAIL: oracle identity hashes were unavailable"
    exit 1
}

IDENTITY_RESULT="$($BIN id --config "$CONFIG" -i "$IDENTITY_HASH" -R -t 5 -p 2>&1)" || {
    echo "FAIL: Prnsd could not resolve the stock identity hash"
    echo "$IDENTITY_RESULT"
    cat "$SERVER_LOG"
    exit 1
}
[[ "$IDENTITY_RESULT" == *"Identity Hash : <$IDENTITY_HASH>"* ]] || {
    echo "FAIL: Prnsd returned the wrong identity for the stock identity hash"
    echo "$IDENTITY_RESULT"
    exit 1
}

DESTINATION_RESULT="$($BIN id --config "$CONFIG" -i "$DESTINATION_HASH" -R -t 5 -p 2>&1)" || {
    echo "FAIL: Prnsd could not resolve the stock destination hash"
    echo "$DESTINATION_RESULT"
    cat "$SERVER_LOG"
    exit 1
}
[[ "$DESTINATION_RESULT" == *"Identity Hash : <$IDENTITY_HASH>"* ]] || {
    echo "FAIL: Prnsd returned the wrong identity for the stock destination hash"
    echo "$DESTINATION_RESULT"
    exit 1
}

set +e
NO_CACHE_RESULT="$($BIN id --config "$CONFIG" -i "$IDENTITY_HASH" -R -N -t 1 -p 2>&1)"
NO_CACHE_STATUS=$?
set -e
[ "$NO_CACHE_STATUS" -eq 2 ] && [[ "$NO_CACHE_RESULT" == *"could not get working identity"* ]] || {
    echo "FAIL: --no-cache did not bypass network identity resolution"
    echo "$NO_CACHE_RESULT"
    echo "exit=$NO_CACHE_STATUS"
    exit 1
}

$BIN id --config "$CONFIG" -i "$ANNOUNCE_IDENTITY" -a oracle.identity > "$WORK/announce.out" 2>&1 || {
    echo "FAIL: Prnsd could not announce through the stock shared instance"
    cat "$WORK/announce.out"
    cat "$SERVER_LOG"
    exit 1
}
for _ in $(seq 1 50); do
    grep -q "RNID_ANNOUNCE_RECEIVED .* $ANNOUNCE_HASH" "$SERVER_LOG" && break
    sleep 0.1
done
grep -q "RNID_ANNOUNCE_RECEIVED .* $ANNOUNCE_HASH" "$SERVER_LOG" || {
    echo "FAIL: stock RNS did not receive the Prnsd identity announcement"
    cat "$WORK/announce.out"
    cat "$SERVER_LOG"
    exit 1
}

echo "PASS: Prnsd id resolved and announced identities through a stock RNS 1.4.0 shared instance"

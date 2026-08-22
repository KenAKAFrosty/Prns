#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
SERVER="$ROOT/validation/interop/peers/rns_link_closure_server.py"
CLIENT="$ROOT/prns-napi/tests/interop/link_closure_client.mjs"
WORK="$(mktemp -d)"
SERVER_LOG="$WORK/server.log"
CLIENT_LOG="$WORK/client.log"
SERVER_PID=""
CLIENT_PID=""

cleanup() {
    for pid in "$CLIENT_PID" "$SERVER_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
    rm -rf -- "$WORK"
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }
command -v node >/dev/null || { echo "FAIL: node is required"; exit 1; }

PORT="$($PYTHON - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"
[ -n "${PORT:-}" ] || { echo "FAIL: could not allocate a port"; exit 1; }

if [ -z "${PRNS_NAPI_PREBUILT:-}" ]; then
    ( cd "$ROOT/prns-napi" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null && npm run build:debug >/dev/null ) || { echo "FAIL: napi addon build"; exit 1; }
fi

PRNS_LINK_CLOSURE_PORT="$PORT" "$PYTHON" "$SERVER" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 100); do
    grep -q "LINK_CLOSURE_SERVER_UP" "$SERVER_LOG" && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "LINK_CLOSURE_SERVER_UP" "$SERVER_LOG" || { echo "FAIL: stock Link closure server did not start"; cat "$SERVER_LOG"; exit 1; }

PRNS_TCP_TARGET="127.0.0.1:$PORT" node "$CLIENT" >"$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!
for _ in $(seq 1 160); do
    grep -q "STOCK_OBSERVED_PRNS_CLOSE reason=initiator" "$SERVER_LOG" && grep -q "STOCK_CLOSED_PRNS_LINK reason=destination" "$SERVER_LOG" && grep -q "NAPI_OBSERVED_STOCK_CLOSE reason=peerClosed" "$CLIENT_LOG" && break
    kill -0 "$CLIENT_PID" 2>/dev/null || break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.25
done

if grep -q "STOCK_OBSERVED_PRNS_CLOSE reason=initiator" "$SERVER_LOG" && grep -q "STOCK_CLOSED_PRNS_LINK reason=destination" "$SERVER_LOG" && grep -q "NAPI_OBSERVED_STOCK_CLOSE reason=peerClosed" "$CLIENT_LOG"; then
    echo "PASS: stock RNS 1.4.2 and Prns each closed an active Link and the remote process observed a clean peer closure"
    exit 0
fi

echo "FAIL: bilateral Link closure evidence did not complete"
cat "$SERVER_LOG"
cat "$CLIENT_LOG"
exit 1

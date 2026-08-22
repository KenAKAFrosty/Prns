#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
SERVER="$ROOT/validation/interop/peers/rns_large_request_server.py"
CLIENT="$ROOT/prns-napi/tests/interop/large_request_client.mjs"
SERVER_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
SERVER_PID=""
CLIENT_PID=""
SERVER_OK="STOCK_LARGE_REQUEST_OK response=131072"
CLIENT_OK="NAPI_LARGE_REQUEST_OK response=131072 responded=131072"

cleanup() {
    for pid in "$CLIENT_PID" "$SERVER_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
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

PRNS_LARGE_REQUEST_PORT="$PORT" "$PYTHON" "$SERVER" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 100); do
    grep -q "LARGE_REQUEST_SERVER_UP" "$SERVER_LOG" && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "LARGE_REQUEST_SERVER_UP" "$SERVER_LOG" || { echo "FAIL: stock RNS large-request server did not start"; cat "$SERVER_LOG"; exit 1; }

PRNS_TCP_TARGET="127.0.0.1:$PORT" node "$CLIENT" >"$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!
for _ in $(seq 1 240); do
    grep -qF "$SERVER_OK" "$SERVER_LOG" && grep -qF "$CLIENT_OK" "$CLIENT_LOG" && break
    if ! kill -0 "$CLIENT_PID" 2>/dev/null; then
        grep -qF "$CLIENT_OK" "$CLIENT_LOG" || break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        grep -qF "$SERVER_OK" "$SERVER_LOG" || break
    fi
    sleep 0.25
done

if grep -qF "$SERVER_OK" "$SERVER_LOG" && grep -qF "$CLIENT_OK" "$CLIENT_LOG"; then
    echo "PASS: stock RNS 1.4.2 and Prns completed Resource-backed Link.request responses in both directions"
    exit 0
fi

echo "FAIL: bidirectional Resource-backed Link.request exchange did not complete"
cat "$SERVER_LOG"
cat "$CLIENT_LOG"
exit 1

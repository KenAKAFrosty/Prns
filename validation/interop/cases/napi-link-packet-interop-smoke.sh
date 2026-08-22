#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
STOCK_PEER="$ROOT/validation/interop/peers/rns_link_packet_peer.py"
NAPI_PEER="$ROOT/prns-napi/tests/interop/link_packet_peer.mjs"
WORK="$(mktemp -d)"
STOCK_LOG="$WORK/stock.log"
NAPI_LOG="$WORK/napi.log"
STOCK_PID=""
NAPI_PID=""
STOCK_OK="STOCK_LINK_PACKET_OK received=1 proof=1"
NAPI_OK="NAPI_LINK_PACKET_OK received=1 proof=1"

cleanup() {
    for pid in "$NAPI_PID" "$STOCK_PID"; do
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

PRNS_LINK_PACKET_PORT="$PORT" \
PRNS_LINK_PACKET_CONFIG_DIR="$WORK/stock-rns" \
    "$PYTHON" "$STOCK_PEER" >"$STOCK_LOG" 2>&1 &
STOCK_PID=$!
for _ in $(seq 1 100); do
    grep -q "LINK_PACKET_PEER_UP" "$STOCK_LOG" && break
    kill -0 "$STOCK_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "LINK_PACKET_PEER_UP" "$STOCK_LOG" || { echo "FAIL: stock Link packet peer did not start"; cat "$STOCK_LOG"; exit 1; }

PRNS_TCP_TARGET="127.0.0.1:$PORT" node "$NAPI_PEER" >"$NAPI_LOG" 2>&1 &
NAPI_PID=$!
for _ in $(seq 1 160); do
    grep -qF "$STOCK_OK" "$STOCK_LOG" && grep -qF "$NAPI_OK" "$NAPI_LOG" && break
    if ! kill -0 "$NAPI_PID" 2>/dev/null; then
        grep -qF "$NAPI_OK" "$NAPI_LOG" || break
    fi
    if ! kill -0 "$STOCK_PID" 2>/dev/null; then
        grep -qF "$STOCK_OK" "$STOCK_LOG" || break
    fi
    sleep 0.25
done

if grep -qF "$STOCK_OK" "$STOCK_LOG" && grep -qF "$NAPI_OK" "$NAPI_LOG"; then
    echo "PASS: stock RNS 1.4.2 and Prns each delivered and proved an exact direct Link packet as responder"
    exit 0
fi

echo "FAIL: bidirectional direct Link packet evidence did not complete"
cat "$STOCK_LOG"
cat "$NAPI_LOG"
exit 1

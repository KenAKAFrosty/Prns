#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
STOCK_PEER="$ROOT/validation/interop/peers/rns_resource_rejection_peer.py"
NAPI_PEER="$ROOT/prns-napi/tests/interop/resource_rejection_peer.mjs"
WORK="$(mktemp -d)"
STOCK_LOG="$WORK/stock-first.log"
NAPI_LOG="$WORK/napi-first.log"
STOCK_PID=""
NAPI_PID=""

stop_peers() {
    for pid in "$NAPI_PID" "$STOCK_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
    STOCK_PID=""
    NAPI_PID=""
}

cleanup() {
    stop_peers
    rm -rf -- "$WORK"
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }
command -v node >/dev/null || { echo "FAIL: node is required"; exit 1; }

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
)"
set -- $PORTS
STOCK_PORT="$1"
NAPI_PORT="$2"

if [ -z "${PRNS_NAPI_PREBUILT:-}" ]; then
    ( cd "$ROOT/prns-napi" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null && npm run build:debug >/dev/null ) || { echo "FAIL: napi addon build"; exit 1; }
fi

PRNS_REJECTION_ROLE=reject-prns PRNS_REJECTION_PORT="$STOCK_PORT" "$PYTHON" "$STOCK_PEER" >"$STOCK_LOG" 2>&1 &
STOCK_PID=$!
for _ in $(seq 1 100); do
    grep -q "STOCK_REJECTION_SERVER_UP" "$STOCK_LOG" && break
    kill -0 "$STOCK_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "STOCK_REJECTION_SERVER_UP" "$STOCK_LOG" || { echo "FAIL: stock rejection server did not start"; cat "$STOCK_LOG"; exit 1; }

PRNS_REJECTION_ROLE=send-to-stock PRNS_TCP_TARGET="127.0.0.1:$STOCK_PORT" node "$NAPI_PEER" >"$NAPI_LOG" 2>&1 &
NAPI_PID=$!
for _ in $(seq 1 160); do
    grep -q "STOCK_REJECTED_PRNS offers=1 published=0" "$STOCK_LOG" && grep -q "NAPI_OBSERVED_STOCK_REJECTION published=0" "$NAPI_LOG" && break
    kill -0 "$NAPI_PID" 2>/dev/null || break
    sleep 0.25
done
if ! grep -q "STOCK_REJECTED_PRNS offers=1 published=0" "$STOCK_LOG" || ! grep -q "NAPI_OBSERVED_STOCK_REJECTION published=0" "$NAPI_LOG"; then
    echo "FAIL: Prns sender did not observe stock RNS Resource rejection"
    cat "$STOCK_LOG"
    cat "$NAPI_LOG"
    exit 1
fi
stop_peers
STOCK_LOG="$WORK/stock-second.log"
NAPI_LOG="$WORK/napi-second.log"

PRNS_REJECTION_ROLE=reject-stock PRNS_TCP_TARGET="127.0.0.1:$NAPI_PORT" node "$NAPI_PEER" >"$NAPI_LOG" 2>&1 &
NAPI_PID=$!
for _ in $(seq 1 100); do
    grep -q "NAPI_REJECTION_SERVER_UP" "$NAPI_LOG" && break
    kill -0 "$NAPI_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "NAPI_REJECTION_SERVER_UP" "$NAPI_LOG" || { echo "FAIL: napi rejection server did not start"; cat "$NAPI_LOG"; exit 1; }

PRNS_REJECTION_ROLE=send-to-prns PRNS_REJECTION_PORT="$NAPI_PORT" "$PYTHON" "$STOCK_PEER" >"$STOCK_LOG" 2>&1 &
STOCK_PID=$!
for _ in $(seq 1 160); do
    grep -q "STOCK_OBSERVED_PRNS_REJECTION progress=0" "$STOCK_LOG" && grep -q "NAPI_REJECTED_STOCK published=0" "$NAPI_LOG" && break
    kill -0 "$STOCK_PID" 2>/dev/null || break
    sleep 0.25
done
if ! grep -q "STOCK_OBSERVED_PRNS_REJECTION progress=0" "$STOCK_LOG" || ! grep -q "NAPI_REJECTED_STOCK published=0" "$NAPI_LOG"; then
    echo "FAIL: stock RNS sender did not observe Prns Resource rejection"
    cat "$STOCK_LOG"
    cat "$NAPI_LOG"
    exit 1
fi

echo "PASS: stock RNS 1.4.2 and Prns explicitly rejected Resource offers in both directions without publishing payload bytes"

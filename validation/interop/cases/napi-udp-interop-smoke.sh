#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
STOCK="$ROOT/validation/interop/peers/rns_udp_peer.py"
CLIENT="$ROOT/prns-napi/tests/interop/udp_peer.mjs"
STOCK_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
STOCK_PID=""
CLIENT_PID=""
STOCK_OK="STOCK_UDP_OK received=1 proven=1"
CLIENT_OK="NAPI_UDP_OK received=1 proven=1"

cleanup() {
    for pid in "$CLIENT_PID" "$STOCK_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }
command -v node >/dev/null || { echo "FAIL: node is required"; exit 1; }

PORTS="$($PYTHON - <<'PY'
import socket

sockets = []
ports = []
for _ in range(2):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
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
PRNS_PORT="$2"

if [ -z "${PRNS_NAPI_PREBUILT:-}" ]; then
    ( cd "$ROOT/prns-napi" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null && npm run build:debug >/dev/null ) || { echo "FAIL: napi addon build"; exit 1; }
fi

RNS_UDP_LOCAL_PORT="$STOCK_PORT" RNS_UDP_PEER_PORT="$PRNS_PORT" "$PYTHON" "$STOCK" >"$STOCK_LOG" 2>&1 &
STOCK_PID=$!
for _ in $(seq 1 100); do
    grep -q "UDP_PEER_UP" "$STOCK_LOG" && break
    kill -0 "$STOCK_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "UDP_PEER_UP" "$STOCK_LOG" || { echo "FAIL: stock RNS UDP peer did not start"; cat "$STOCK_LOG"; exit 1; }

PRNS_UDP_LOCAL="127.0.0.1:$PRNS_PORT" PRNS_UDP_PEER="127.0.0.1:$STOCK_PORT" node "$CLIENT" >"$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!
for _ in $(seq 1 160); do
    grep -qF "$STOCK_OK" "$STOCK_LOG" && grep -qF "$CLIENT_OK" "$CLIENT_LOG" && break
    if ! kill -0 "$CLIENT_PID" 2>/dev/null; then
        grep -qF "$CLIENT_OK" "$CLIENT_LOG" || break
    fi
    if ! kill -0 "$STOCK_PID" 2>/dev/null; then
        grep -qF "$STOCK_OK" "$STOCK_LOG" || break
    fi
    sleep 0.25
done

if grep -qF "$STOCK_OK" "$STOCK_LOG" && grep -qF "$CLIENT_OK" "$CLIENT_LOG"; then
    echo "PASS: stock RNS 1.4.2 UDPInterface and Prns UDP exchanged exact proven packets in both directions"
    exit 0
fi

echo "FAIL: bidirectional stock-RNS UDP exchange did not complete"
cat "$STOCK_LOG"
cat "$CLIENT_LOG"
exit 1

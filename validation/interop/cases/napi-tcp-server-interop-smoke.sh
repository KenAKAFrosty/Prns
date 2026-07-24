#!/usr/bin/env bash
# Stock RNS dials the Node addon's TCP server and proves a packet in both directions.
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/benchmarks/reference/.venv/bin/python}"
CLIENT="$ROOT/validation/interop/peers/rns_tcp_client_peer.py"
HOST_DRIVER="$ROOT/prns-napi/tests/interop/tcp_server_host.mjs"
HOST_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
HOST_PID=""
CLIENT_PID=""

cleanup() {
    for pid in "$CLIENT_PID" "$HOST_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    wait "$CLIENT_PID" "$HOST_PID" 2>/dev/null
}
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }
command -v node >/dev/null || { echo "FAIL: node is required"; exit 1; }

PORT="$("$VENV_PY" - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()
PY
)"
[ -n "${PORT:-}" ] || { echo "FAIL: could not allocate a port"; exit 1; }
echo "napi TcpServer port=$PORT"

if [ -z "${PRNS_NAPI_PREBUILT:-}" ]; then
    echo "building the napi addon..."
    ( cd "$ROOT/prns-napi" && npm ci --ignore-scripts --no-audit --no-fund > /dev/null && npm run build:debug > /dev/null ) \
        || { echo "FAIL: napi addon build"; exit 1; }
fi

PORT="$PORT" node "$HOST_DRIVER" > "$HOST_LOG" 2>&1 &
HOST_PID=$!
for _ in $(seq 1 100); do grep -q "listening on" "$HOST_LOG" && break; sleep 0.1; done
grep -q "listening on" "$HOST_LOG" || { echo "FAIL: napi tcp_server_host never bound"; cat "$HOST_LOG"; exit 1; }
echo "napi TcpServer up"

PRNS_TCP_TARGET="127.0.0.1:$PORT" "$VENV_PY" "$CLIENT" > "$CLIENT_LOG" 2>/dev/null &
CLIENT_PID=$!

for _ in $(seq 1 160); do
    grep -q "PROVEN" "$CLIENT_LOG" && break
    kill -0 "$CLIENT_PID" 2>/dev/null || break
    sleep 0.25
done

if grep -q "PROVEN" "$CLIENT_LOG"; then
    echo "PASS: stock RNS TCPClientInterface linked the napi TcpServer; announce heard and a single proven both ways"
    echo "  heard: $(grep -o 'HEARD_HOST .*' "$CLIENT_LOG" | head -1)"
    exit 0
fi

echo "FAIL: stock RNS client did not get a proof from the napi TcpServer"
echo "--- client log ---"; tail -20 "$CLIENT_LOG"
echo "--- host log ---"; tail -20 "$HOST_LOG"
exit 1

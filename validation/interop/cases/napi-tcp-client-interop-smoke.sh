#!/usr/bin/env bash
# Direction-A TCP parity smoke for the Node addon: the napi TcpClient dials stock RNS.
#
# Stands up a stock RNS node whose only interface is a `TCPServerInterface` hosting a ProveAll
# `hopspot.host` destination it announces (`rns_tcp_server_peer.py`), then the Node addon's
# tcp_client_probe driver dialing it. The napi client hears the announce, sends the destination a
# single, and awaits the stock node's proof — the napi `sendSinglePacket` resolving with proof
# evidence is the pass condition, exercising the binding's TcpClient in both directions.
#
# The reference RNS is the pinned venv (benchmarks/reference; $SMOKE_PYTHON if set). Prints PASS or
# FAIL and exits accordingly. Set PRNS_NAPI_PREBUILT=1 to skip the npm build when the addon binary
# already sits in prns-napi/.
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/benchmarks/reference/.venv/bin/python}"
SERVER="$ROOT/validation/interop/peers/rns_tcp_server_peer.py"
CLIENT_DRIVER="$ROOT/prns-napi/tests/interop/tcp_client_probe.mjs"
SERVER_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
SERVER_PID=""
CLIENT_PID=""

cleanup() {
    for pid in "$CLIENT_PID" "$SERVER_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    wait "$CLIENT_PID" "$SERVER_PID" 2>/dev/null
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
echo "stock TCPServerInterface port=$PORT"

if [ -z "${PRNS_NAPI_PREBUILT:-}" ]; then
    echo "building the napi addon..."
    ( cd "$ROOT/prns-napi" && npm ci --ignore-scripts --no-audit --no-fund > /dev/null && npm run build:debug > /dev/null ) \
        || { echo "FAIL: napi addon build"; exit 1; }
fi

# 1) Stock RNS: a TCPServerInterface hosting a ProveAll destination it announces.
PRNS_TCP_LISTEN_PORT="$PORT" "$VENV_PY" "$SERVER" > "$SERVER_LOG" 2>/dev/null &
SERVER_PID=$!
for _ in $(seq 1 100); do grep -q "SERVER_UP" "$SERVER_LOG" && break; sleep 0.1; done
grep -q "SERVER_UP" "$SERVER_LOG" || { echo "FAIL: stock TCP server never came up"; cat "$SERVER_LOG"; exit 1; }
echo "stock TCP server up"

# 2) The napi client: dial, hear the announce, send a single, await the proof.
PRNS_TCP_TARGET="127.0.0.1:$PORT" node "$CLIENT_DRIVER" > "$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!

for _ in $(seq 1 160); do
    grep -q "PROVEN" "$CLIENT_LOG" && break
    kill -0 "$CLIENT_PID" 2>/dev/null || break
    sleep 0.25
done

if grep -q "PROVEN" "$CLIENT_LOG"; then
    echo "PASS: the napi TcpClient linked stock RNS's TCPServerInterface; announce heard and a single proven both ways"
    echo "  heard: $(grep -o 'HEARD_HOST .*' "$CLIENT_LOG" | head -1)"
    exit 0
fi

echo "FAIL: the napi client did not get a proof from the stock TCPServerInterface"
echo "--- napi client log ---"; tail -20 "$CLIENT_LOG"
echo "--- stock server log ---"; tail -20 "$SERVER_LOG"
exit 1

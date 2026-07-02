#!/usr/bin/env bash
# Direction-B TCP parity smoke: stock RNS dials *our* TcpServer.
#
# Stands up the Prns `tcp_server_host` example (a `TcpServer` hosting a ProveAll `hopspot.host`
# destination it announces), then a stock RNS node whose only interface is a `TCPClientInterface`
# pointed at it. The stock node hears our announce (announce carried outbound over a stock RNS client
# link), sends our destination a single (inbound data), and the ProveAll proof comes back (outbound) —
# one proven round trip exercising our `TcpServer` against stock RNS's `TCPClientInterface` in both
# directions. The inverse of local-transit-smoke, which already proves our `TcpClient` against stock
# RNS's `TCPServerInterface`. Together they cover the directive's "RNS parity for TCP/IP, server and
# client."
#
# The reference RNS is the pinned venv (benchmarks/reference; $SMOKE_PYTHON if set). Prints PASS or
# FAIL and exits accordingly.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/benchmarks/reference/.venv/bin/python}"
CLIENT="$ROOT/prns-core/tests/interop/rns_tcp_client_peer.py"
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

PORT="$("$VENV_PY" - <<'PY'
import socket
s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()
PY
)"
[ -n "${PORT:-}" ] || { echo "FAIL: could not allocate a port"; exit 1; }
echo "our TcpServer port=$PORT"

echo "building the tcp_server_host example..."
( cd "$ROOT/prns-interfaces/tokio" && cargo build --quiet --example tcp_server_host --features tcp ) \
    || { echo "FAIL: tcp_server_host build"; exit 1; }

# 1) Our node: a TcpServer hosting a ProveAll destination it announces.
PORT="$PORT" "$ROOT/prns-interfaces/tokio/target/debug/examples/tcp_server_host" > "$HOST_LOG" 2>&1 &
HOST_PID=$!
for _ in $(seq 1 100); do grep -q "listening on" "$HOST_LOG" && break; sleep 0.1; done
grep -q "listening on" "$HOST_LOG" || { echo "FAIL: tcp_server_host never bound"; cat "$HOST_LOG"; exit 1; }
echo "our TcpServer up"

# 2) Stock RNS, TCPClientInterface dialing us: hear the announce, send a single, await the proof.
PRNS_TCP_TARGET="127.0.0.1:$PORT" "$VENV_PY" "$CLIENT" > "$CLIENT_LOG" 2>/dev/null &
CLIENT_PID=$!

for _ in $(seq 1 160); do
    grep -q "PROVEN" "$CLIENT_LOG" && break
    kill -0 "$CLIENT_PID" 2>/dev/null || break
    sleep 0.25
done

if grep -q "PROVEN" "$CLIENT_LOG"; then
    echo "PASS: stock RNS TCPClientInterface linked our TcpServer; announce heard and a single proven both ways"
    echo "  heard: $(grep -o 'HEARD_HOST .*' "$CLIENT_LOG" | head -1)"
    exit 0
fi

echo "FAIL: stock RNS client did not get a proof from our TcpServer"
echo "--- client log ---"; tail -20 "$CLIENT_LOG"
echo "--- host log ---"; tail -20 "$HOST_LOG"
exit 1

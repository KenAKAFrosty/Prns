#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$ROOT/validation/interop/lib/cargo-artifacts.sh"
PYTHON="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
DAEMON="$(cargo_debug_example "$ROOT/validation/integration/Cargo.toml" mixed_multihop_daemon)"
TRANSPORT="$ROOT/validation/interop/peers/rns_multihop_transport.py"
ENDPOINT="$ROOT/validation/interop/peers/rns_multihop_endpoint.py"
RIGHT_LOG="$(mktemp)"
PRNS_LOG="$(mktemp)"
TRANSPORT_LOG="$(mktemp)"
LEFT_LOG="$(mktemp)"
RIGHT_PID=""
PRNS_PID=""
TRANSPORT_PID=""
LEFT_PID=""
LEFT_OK="MULTIHOP_OK role=left hops=3 bytes=65536"
RIGHT_OK="MULTIHOP_OK role=right hops=3 bytes=65536"

cleanup() {
    for pid in "$LEFT_PID" "$TRANSPORT_PID" "$PRNS_PID" "$RIGHT_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        [ -n "$pid" ] && wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }

PORTS="$($PYTHON - <<'PY'
import socket

sockets = []
ports = []
for _ in range(3):
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
LEFT_PORT="$1"
PRNS_PORT="$2"
RIGHT_PORT="$3"

cargo build --quiet --manifest-path "$ROOT/validation/integration/Cargo.toml" --example mixed_multihop_daemon || { echo "FAIL: mixed multi-hop daemon build"; exit 1; }

RNS_MULTIHOP_ROLE=right RNS_MULTIHOP_ENDPOINT_PORT="$RIGHT_PORT" "$PYTHON" "$ENDPOINT" >"$RIGHT_LOG" 2>&1 &
RIGHT_PID=$!
for _ in $(seq 1 100); do
    grep -q "MULTIHOP_ENDPOINT_UP role=right" "$RIGHT_LOG" && break
    kill -0 "$RIGHT_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "MULTIHOP_ENDPOINT_UP role=right" "$RIGHT_LOG" || { echo "FAIL: right stock RNS endpoint did not start"; cat "$RIGHT_LOG"; exit 1; }

PRNS_MULTIHOP_LISTEN_PORT="$PRNS_PORT" PRNS_MULTIHOP_PEER="127.0.0.1:$RIGHT_PORT" "$DAEMON" >"$PRNS_LOG" 2>&1 &
PRNS_PID=$!
for _ in $(seq 1 100); do
    grep -q "MIXED_MULTIHOP_READY" "$PRNS_LOG" && break
    kill -0 "$PRNS_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "MIXED_MULTIHOP_READY" "$PRNS_LOG" || { echo "FAIL: Prns transport did not start"; cat "$PRNS_LOG"; exit 1; }

RNS_MULTIHOP_LISTEN_PORT="$LEFT_PORT" RNS_MULTIHOP_PEER_PORT="$PRNS_PORT" "$PYTHON" "$TRANSPORT" >"$TRANSPORT_LOG" 2>&1 &
TRANSPORT_PID=$!
for _ in $(seq 1 100); do
    grep -q "MULTIHOP_TRANSPORT_UP" "$TRANSPORT_LOG" && break
    kill -0 "$TRANSPORT_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "MULTIHOP_TRANSPORT_UP" "$TRANSPORT_LOG" || { echo "FAIL: stock RNS transport did not start"; cat "$TRANSPORT_LOG"; exit 1; }

RNS_MULTIHOP_ROLE=left RNS_MULTIHOP_ENDPOINT_PORT="$LEFT_PORT" "$PYTHON" "$ENDPOINT" >"$LEFT_LOG" 2>&1 &
LEFT_PID=$!

for _ in $(seq 1 400); do
    grep -qF "$LEFT_OK" "$LEFT_LOG" && grep -qF "$RIGHT_OK" "$RIGHT_LOG" && break
    if ! kill -0 "$LEFT_PID" 2>/dev/null; then
        grep -qF "$LEFT_OK" "$LEFT_LOG" || break
    fi
    if ! kill -0 "$RIGHT_PID" 2>/dev/null; then
        grep -qF "$RIGHT_OK" "$RIGHT_LOG" || break
    fi
    kill -0 "$TRANSPORT_PID" 2>/dev/null || break
    kill -0 "$PRNS_PID" 2>/dev/null || break
    sleep 0.25
done

if grep -qF "$LEFT_OK" "$LEFT_LOG" && grep -qF "$RIGHT_OK" "$RIGHT_LOG"; then
    echo "PASS: stock RNS endpoints exchanged exact Resources across stock and Prns transport nodes in series at three reported path hops"
    exit 0
fi

echo "FAIL: stable mixed two-transport topology did not complete"
cat "$LEFT_LOG"
cat "$RIGHT_LOG"
cat "$TRANSPORT_LOG"
cat "$PRNS_LOG"
exit 1

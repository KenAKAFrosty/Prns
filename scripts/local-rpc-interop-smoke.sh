#!/usr/bin/env bash
# RNS 1.3.5 shared-instance control-RPC smoke.
#
# Stands up a Prns-owned LocalServer plus the RPC compatibility shim, then lets
# a stock RNS 1.3.5 client connect and call Reticulum's own get_* methods. That
# exercises the modern msgpack RPC dialect end to end.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/benchmarks/reference/.venv/bin/python}"
CLIENT="$ROOT/personal-rns/tests/interop/rns_shared_rpc_client.py"
RPC_KEY="${PRNS_RPC_KEY:-$(printf '5a%.0s' $(seq 1 32))}"
DAEMON_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
DAEMON_PID=""

cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    [ -n "$DAEMON_PID" ] && wait "$DAEMON_PID" 2>/dev/null
}
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }

if [ -n "${PRNS_LOCAL_PORT:-}" ] && [ -n "${PRNS_RPC_PORT:-}" ]; then
    LOCAL_PORT="$PRNS_LOCAL_PORT"
    RPC_PORT="$PRNS_RPC_PORT"
else
    PORTS="$("$VENV_PY" - <<'PY'
import socket

sockets = []
ports = []
for _ in range(2):
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    sockets.append(sock)
    ports.append(sock.getsockname()[1])
print(ports[0], ports[1])
for sock in sockets:
    sock.close()
PY
)" || { echo "FAIL: could not allocate loopback ports"; exit 1; }
    set -- $PORTS
    LOCAL_PORT="${1:-}"
    RPC_PORT="${2:-}"
fi

[ -n "${LOCAL_PORT:-}" ] && [ -n "${RPC_PORT:-}" ] || {
    echo "FAIL: empty shared-instance or RPC port"
    exit 1
}

echo "building the shared-instance RPC daemon example..."
( cd "$ROOT" && cargo build --quiet --example local_shared_rpc_instance --features local ) \
    || { echo "FAIL: daemon build"; exit 1; }

PRNS_LOCAL_PORT="$LOCAL_PORT" \
PRNS_RPC_PORT="$RPC_PORT" \
PRNS_RPC_KEY="$RPC_KEY" \
"$ROOT/target/debug/examples/local_shared_rpc_instance" > "$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 50); do grep -q "READY" "$DAEMON_LOG" && break; sleep 0.2; done
grep -q "READY" "$DAEMON_LOG" || { echo "FAIL: daemon never became READY"; cat "$DAEMON_LOG"; exit 1; }
echo "daemon ready; running the RNS 1.3.5 RPC oracle..."

PRNS_LOCAL_PORT="$LOCAL_PORT" \
PRNS_RPC_PORT="$RPC_PORT" \
PRNS_RPC_KEY="$RPC_KEY" \
"$VENV_PY" "$CLIENT" > "$CLIENT_LOG" 2>&1
status=$?

if [ "$status" -eq 0 ] && grep -q "RPC_ORACLE_OK" "$CLIENT_LOG"; then
    echo "PASS: stock RNS 1.3.5 decoded Prns msgpack control-RPC replies"
    grep "RPC_ORACLE_OK" "$CLIENT_LOG" | head -1
    exit 0
fi

echo "FAIL: RNS 1.3.5 RPC oracle failed"
echo "--- client log ---"; cat "$CLIENT_LOG"
echo "--- daemon log ---"; tail -30 "$DAEMON_LOG"
exit 1

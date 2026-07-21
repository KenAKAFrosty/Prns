#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/benchmarks/reference/.rpc-venv/bin/python}"
RNX="$(dirname "$PYTHON")/rnx"
ORACLE="$ROOT/prns-core/tests/interop/rns_rnx_oracle.py"
BIN="$ROOT/prnsd/target/debug/prnsd"
WORK="$(mktemp -d)"
CONFIG="$WORK/config"
CLIENT_CONFIG="$WORK/client-config"
STOCK_ID="$WORK/stock.rid"
CLIENT_ID="$WORK/client.rid"
PRNS_ID="$WORK/prns.rid"
RNSD_LOG="$WORK/server.log"
LISTENER_LOG="$WORK/listener.log"
RNSD_PID=""
LISTENER_PID=""

cleanup() {
    STATUS=$?
    [ -n "$LISTENER_PID" ] && kill "$LISTENER_PID" 2>/dev/null || true
    [ -n "$LISTENER_PID" ] && wait "$LISTENER_PID" 2>/dev/null || true
    [ -n "$RNSD_PID" ] && kill "$RNSD_PID" 2>/dev/null || true
    [ -n "$RNSD_PID" ] && wait "$RNSD_PID" 2>/dev/null || true
    if [ "$STATUS" -ne 0 ]; then
        [ -f "$RNSD_LOG" ] && tail -n 200 "$RNSD_LOG"
        [ -f "$LISTENER_LOG" ] && tail -n 200 "$LISTENER_LOG"
    fi
    if [ "${RNX_KEEP_WORK:-0}" = "1" ]; then
        echo "RNX work preserved at $WORK"
    else
        rm -rf -- "$WORK"
    fi
    exit "$STATUS"
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }
[ -x "$RNX" ] || { echo "FAIL: stock rnx not found at $RNX"; exit 1; }

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
BUS_PORT="$1"
CONTROL_PORT="$2"
NETWORK_PORT="$3"
STOCK_DESTINATION="$($PYTHON "$ORACLE" prepare "$CONFIG" "$CLIENT_CONFIG" "$BUS_PORT" "$CONTROL_PORT" "$NETWORK_PORT" "$STOCK_ID" "$CLIENT_ID")"

( cd "$ROOT/prnsd" && cargo build --quiet )
"$PYTHON" "$ORACLE" serve "$CONFIG" "$STOCK_ID" > "$RNSD_LOG" 2>&1 &
RNSD_PID=$!
for _ in $(seq 1 100); do
    grep -q "RNX_SERVER_READY" "$RNSD_LOG" && break
    kill -0 "$RNSD_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNX_SERVER_READY $STOCK_DESTINATION" "$RNSD_LOG" || { echo "FAIL: stock RNX server stopped"; exit 1; }

set +e
"$BIN" x --config "$CONFIG" -i "$PRNS_ID" -m --stdin payload "$STOCK_DESTINATION" "oracle-command" > "$WORK/prns-client.out" 2> "$WORK/prns-client.err"
PRNS_CLIENT_STATUS=$?
set -e
[ "$PRNS_CLIENT_STATUS" -eq 7 ] || { echo "FAIL: Prns x did not mirror the stock result"; cat "$WORK/prns-client.out"; cat "$WORK/prns-client.err"; exit 1; }
grep -q "identified:oracle-command:payload" "$WORK/prns-client.out" || { echo "FAIL: stock RNX did not decode the Prns request"; cat "$WORK/prns-client.out"; exit 1; }
grep -q "stock-stderr" "$WORK/prns-client.err" || { echo "FAIL: Prns x did not decode stock stderr"; cat "$WORK/prns-client.err"; exit 1; }

PRNS_DESTINATION="$($BIN x --config "$CONFIG" -i "$PRNS_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$PRNS_DESTINATION" ] || { echo "FAIL: Prns listener destination unavailable"; exit 1; }
CLIENT_HASH="$($PYTHON "$ORACLE" identity-hash "$CLIENT_ID")"
"$BIN" x --config "$CONFIG" -i "$PRNS_ID" -l -a "$CLIENT_HASH" > "$LISTENER_LOG" 2>&1 &
LISTENER_PID=$!
sleep 0.5
"$RNX" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -w 5 "$PRNS_DESTINATION" "printf stock-to-prns" > "$WORK/stock-client.out" 2> "$WORK/stock-client.err"
grep -q "stock-to-prns" "$WORK/stock-client.out" || { echo "FAIL: stock rnx did not decode the Prns response"; cat "$WORK/stock-client.out"; exit 1; }

set +e
"$PYTHON" - "$RNX" --config "$CLIENT_CONFIG" -i "$STOCK_ID" -w 2 "$PRNS_DESTINATION" "printf denied" > "$WORK/denied.out" 2>&1 <<'PY'
import subprocess
import sys

try:
    result = subprocess.run(sys.argv[1:], timeout=8)
except subprocess.TimeoutExpired:
    sys.exit(124)
sys.exit(result.returncode)
PY
DENIED_STATUS=$?
set -e
[ "$DENIED_STATUS" -ne 0 ] || { echo "FAIL: unlisted stock x client was accepted"; cat "$WORK/denied.out"; exit 1; }

echo "PASS: Prnsd x exchanges authenticated execution requests and results with stock RNS 1.3.9 rnx"

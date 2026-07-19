#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/benchmarks/reference/.rpc-venv/bin/python}"
RNCP="$(dirname "$PYTHON")/rncp"
ORACLE="$ROOT/prns-core/tests/interop/rns_rncp_oracle.py"
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
        [ -f "$RNSD_LOG" ] && cat "$RNSD_LOG"
        [ -f "$LISTENER_LOG" ] && cat "$LISTENER_LOG"
    fi
    if [ "${RNCP_KEEP_WORK:-0}" = "1" ]; then
        echo "RNCP work preserved at $WORK"
    else
        rm -rf -- "$WORK"
    fi
    exit "$STATUS"
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }
[ -x "$RNCP" ] || { echo "FAIL: stock rncp not found at $RNCP"; exit 1; }

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
mkdir -p "$WORK/stock-receive" "$WORK/prns-receive" "$WORK/stock-fetch" "$WORK/prns-fetch" "$WORK/stock-fetched" "$WORK/prns-fetched" "$WORK/auth-receive" "$WORK/auth-fetched"
"$PYTHON" "$ORACLE" serve "$CONFIG" "$STOCK_ID" "$WORK/stock-receive" "$WORK/stock-fetch" > "$RNSD_LOG" 2>&1 &
RNSD_PID=$!
for _ in $(seq 1 100); do
    grep -q "RNCP_SERVER_READY" "$RNSD_LOG" && break
    kill -0 "$RNSD_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNCP_SERVER_READY $STOCK_DESTINATION" "$RNSD_LOG" || { echo "FAIL: stock RNCP server stopped"; cat "$RNSD_LOG"; exit 1; }

"$PYTHON" -c 'import pathlib,sys; pathlib.Path(sys.argv[1]).write_bytes(b"prns-to-stock\n" * 12000); pathlib.Path(sys.argv[2]).write_bytes(b"stock-to-prns\n" * 12000); pathlib.Path(sys.argv[3]).write_bytes(b"served-by-stock\n" * 12000); pathlib.Path(sys.argv[4]).write_bytes(b"served-by-prns\n" * 12000)' "$WORK/prns-send.bin" "$WORK/stock-send.bin" "$WORK/stock-fetch/stock.txt" "$WORK/prns-fetch/prns.txt"

"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -S -P "$WORK/prns-send.bin" "$STOCK_DESTINATION"
for _ in $(seq 1 100); do
    [ -f "$WORK/stock-receive/prns-send.bin" ] && break
    sleep 0.1
done
cmp "$WORK/prns-send.bin" "$WORK/stock-receive/prns-send.bin" || { echo "FAIL: stock rncp did not receive Prns bytes"; cat "$LISTENER_LOG"; exit 1; }

PRNS_DESTINATION="$($BIN cp --config "$CONFIG" -i "$PRNS_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$PRNS_DESTINATION" ] || { echo "FAIL: Prns listener destination unavailable"; exit 1; }
"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -l -n -s "$WORK/prns-receive" > "$LISTENER_LOG" 2>&1 &
LISTENER_PID=$!
sleep 0.5
"$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -S "$WORK/stock-send.bin" "$PRNS_DESTINATION"
for _ in $(seq 1 100); do
    [ -f "$WORK/prns-receive/stock-send.bin" ] && break
    sleep 0.1
done
cmp "$WORK/stock-send.bin" "$WORK/prns-receive/stock-send.bin" || { echo "FAIL: Prns did not receive stock rncp bytes"; cat "$LISTENER_LOG"; exit 1; }
kill "$LISTENER_PID"
wait "$LISTENER_PID" 2>/dev/null || true
LISTENER_PID=""

"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -S -P -f -s "$WORK/prns-fetched" stock.txt "$STOCK_DESTINATION"
cmp "$WORK/stock-fetch/stock.txt" "$WORK/prns-fetched/stock.txt" || { echo "FAIL: Prns did not fetch stock rncp bytes"; cat "$LISTENER_LOG"; exit 1; }

"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -l -n -F -j "$WORK/prns-fetch" > "$LISTENER_LOG" 2>&1 &
LISTENER_PID=$!
sleep 0.5
"$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -S -f -s "$WORK/stock-fetched" prns.txt "$PRNS_DESTINATION"
cmp "$WORK/prns-fetch/prns.txt" "$WORK/stock-fetched/prns.txt" || { echo "FAIL: stock rncp did not fetch Prns bytes"; cat "$LISTENER_LOG"; exit 1; }
kill "$LISTENER_PID"
wait "$LISTENER_PID" 2>/dev/null || true
LISTENER_PID=""

CLIENT_HASH="$($PYTHON "$ORACLE" identity-hash "$CLIENT_ID")"
"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -l -F -a "$CLIENT_HASH" -s "$WORK/auth-receive" -j "$WORK/prns-fetch" > "$LISTENER_LOG" 2>&1 &
LISTENER_PID=$!
sleep 0.5
"$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -S "$WORK/stock-send.bin" "$PRNS_DESTINATION"
for _ in $(seq 1 100); do
    [ -f "$WORK/auth-receive/stock-send.bin" ] && break
    sleep 0.1
done
cmp "$WORK/stock-send.bin" "$WORK/auth-receive/stock-send.bin" || { echo "FAIL: authenticated stock sender was rejected"; cat "$LISTENER_LOG"; exit 1; }
"$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -S -f -s "$WORK/auth-fetched" prns.txt "$PRNS_DESTINATION"
cmp "$WORK/prns-fetch/prns.txt" "$WORK/auth-fetched/prns.txt" || { echo "FAIL: authenticated stock fetch was rejected"; cat "$LISTENER_LOG"; exit 1; }
set +e
"$PYTHON" - "$RNCP" --config "$CLIENT_CONFIG" -i "$STOCK_ID" -S -w 5 "$WORK/prns-send.bin" "$PRNS_DESTINATION" > "$WORK/denied.out" 2>&1 <<'PY'
import subprocess
import sys

try:
    result = subprocess.run(sys.argv[1:], timeout=10)
except subprocess.TimeoutExpired:
    sys.exit(124)
sys.exit(result.returncode)
PY
DENIED_STATUS=$?
set -e
[ "$DENIED_STATUS" -ne 0 ] || { echo "FAIL: unlisted stock sender was accepted"; cat "$WORK/denied.out"; exit 1; }
[ ! -f "$WORK/auth-receive/prns-send.bin" ] || { echo "FAIL: unlisted stock bytes were published"; exit 1; }

echo "PASS: Prnsd cp sends, receives, serves, and fetches with stock RNS 1.3.8 rncp"

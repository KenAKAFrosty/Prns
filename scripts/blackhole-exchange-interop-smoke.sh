#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/benchmarks/reference/.rpc-venv/bin/python}"
CLIENT="$ROOT/prns-core/tests/interop/rns_blackhole_exchange.py"
WORK="$(mktemp -d)"
PRNS_PID=""
STOCK_PID=""

cleanup() {
    [ -n "$PRNS_PID" ] && kill "$PRNS_PID" 2>/dev/null
    [ -n "$PRNS_PID" ] && wait "$PRNS_PID" 2>/dev/null
    [ -n "$STOCK_PID" ] && kill "$STOCK_PID" 2>/dev/null
    [ -n "$STOCK_PID" ] && wait "$STOCK_PID" 2>/dev/null
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }

free_port() {
    "$PYTHON" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

wait_for_port() {
    local port="$1"
    for _ in $(seq 1 150); do
        "$PYTHON" -c 'import socket, sys; sock = socket.socket(); sock.settimeout(0.1); sys.exit(sock.connect_ex(("127.0.0.1", int(sys.argv[1]))))' "$port" && return 0
        sleep 0.1
    done
    return 1
}

( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }

PUBLISHER_SERVER="$WORK/prns-publisher"
PUBLISHER_CLIENT="$WORK/stock-client"
PUBLISHER_PORT="$(free_port)"
PUBLISHER_SOURCE="$($PYTHON "$CLIENT" prepare-prns-publisher "$PUBLISHER_SERVER" "$PUBLISHER_CLIENT" "$PUBLISHER_PORT")"
"$PRNSD" run --log-format json --config "$PUBLISHER_SERVER" &
PRNS_PID=$!
wait_for_port "$PUBLISHER_PORT" || { echo "FAIL: Prnsd publisher listener never became ready"; exit 1; }
PUBLISHER_RESULT="$($PYTHON "$CLIENT" query "$PUBLISHER_CLIENT" "$PUBLISHER_SOURCE" 2>&1)"
if [[ "$PUBLISHER_RESULT" != *"BLACKHOLE_PUBLISHER_OK"* ]]; then
    echo "FAIL: stock RNS did not receive Prnsd's blackhole list"
    echo "$PUBLISHER_RESULT"
    exit 1
fi
kill "$PRNS_PID" 2>/dev/null
wait "$PRNS_PID" 2>/dev/null
PRNS_PID=""

STOCK_SERVER="$WORK/stock-publisher"
PRNS_CLIENT="$WORK/prns-client"
STOCK_PORT="$(free_port)"
STOCK_SOURCE="$($PYTHON "$CLIENT" prepare-stock-publisher "$STOCK_SERVER" "$PRNS_CLIENT" "$STOCK_PORT")"
"$PYTHON" "$CLIENT" serve "$STOCK_SERVER" &
STOCK_PID=$!
wait_for_port "$STOCK_PORT" || { echo "FAIL: stock RNS publisher listener never became ready"; exit 1; }
"$PRNSD" run --log-format json --config "$PRNS_CLIENT" &
PRNS_PID=$!
SOURCE_FILE="$PRNS_CLIENT/storage/blackhole/$STOCK_SOURCE"
for _ in $(seq 1 500); do
    [ -f "$SOURCE_FILE" ] && break
    kill -0 "$PRNS_PID" 2>/dev/null || break
    sleep 0.1
done
[ -f "$SOURCE_FILE" ] || { echo "FAIL: Prnsd did not persist the stock RNS source list"; exit 1; }
UPDATER_RESULT="$($PYTHON "$CLIENT" verify-source-file "$SOURCE_FILE" "$STOCK_SOURCE" 2>&1)"
if [[ "$UPDATER_RESULT" != *"BLACKHOLE_UPDATER_OK"* ]]; then
    echo "FAIL: Prnsd's imported source file was not stock-compatible"
    echo "$UPDATER_RESULT"
    exit 1
fi

echo "PASS: stock RNS 1.3.8 fetched Prnsd's blackhole list"
echo "$PUBLISHER_RESULT" | grep "BLACKHOLE_PUBLISHER_OK"
echo "PASS: Prnsd fetched and persisted stock RNS 1.3.8's blackhole list"
echo "$UPDATER_RESULT" | grep "BLACKHOLE_UPDATER_OK"

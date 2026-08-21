#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$ROOT/validation/interop/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/validation/.venv/rns-rpc-1.4.2/bin/python}"
CLIENT="$ROOT/validation/interop/peers/rns_blackhole_exchange.py"
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

wait_for_marker() {
    local pid="$1"
    local log="$2"
    local marker="$3"
    local failure="$4"
    for _ in $(seq 1 150); do
        grep -Fq "$marker" "$log" && return 0
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    echo "FAIL: $failure"
    tail -30 "$log"
    return 1
}

( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }

PUBLISHER_SERVER="$WORK/prns-publisher"
PUBLISHER_CLIENT="$WORK/stock-client"
PUBLISHER_LOG="$WORK/prns-publisher.log"
PUBLISHER_PORT="$(free_port)"
PUBLISHER_SOURCE="$($PYTHON "$CLIENT" prepare-prns-publisher "$PUBLISHER_SERVER" "$PUBLISHER_CLIENT" "$PUBLISHER_PORT")"
RUST_LOG=info "$PRNSD" run --log-format json --config "$PUBLISHER_SERVER" > "$PUBLISHER_LOG" 2>&1 &
PRNS_PID=$!
wait_for_marker "$PRNS_PID" "$PUBLISHER_LOG" '"event":"daemon_ready' "Prnsd publisher never became ready" || exit 1
PUBLISHER_RESULT="$($PYTHON "$CLIENT" query "$PUBLISHER_CLIENT" "$PUBLISHER_SOURCE" 2>&1)"
if [[ "$PUBLISHER_RESULT" != *"BLACKHOLE_PUBLISHER_OK"* ]]; then
    echo "FAIL: stock RNS did not receive Prnsd's blackhole list"
    echo "$PUBLISHER_RESULT"
    tail -30 "$PUBLISHER_LOG"
    exit 1
fi
kill "$PRNS_PID" 2>/dev/null
wait "$PRNS_PID" 2>/dev/null
PRNS_PID=""

STOCK_SERVER="$WORK/stock-publisher"
PRNS_CLIENT="$WORK/prns-client"
STOCK_LOG="$WORK/stock-publisher.log"
UPDATER_LOG="$WORK/prns-updater.log"
STOCK_PORT="$(free_port)"
STOCK_SOURCE="$($PYTHON "$CLIENT" prepare-stock-publisher "$STOCK_SERVER" "$PRNS_CLIENT" "$STOCK_PORT")"
"$PYTHON" "$CLIENT" serve "$STOCK_SERVER" > "$STOCK_LOG" 2>&1 &
STOCK_PID=$!
wait_for_marker "$STOCK_PID" "$STOCK_LOG" "BLACKHOLE_SERVER_READY" "stock RNS publisher never became ready" || exit 1
RUST_LOG=info "$PRNSD" run --log-format json --config "$PRNS_CLIENT" > "$UPDATER_LOG" 2>&1 &
PRNS_PID=$!
wait_for_marker "$PRNS_PID" "$UPDATER_LOG" '"event":"daemon_ready' "Prnsd updater never became ready" || exit 1
SOURCE_FILE="$PRNS_CLIENT/storage/blackhole/$STOCK_SOURCE"
for _ in $(seq 1 500); do
    [ -f "$SOURCE_FILE" ] && break
    kill -0 "$PRNS_PID" 2>/dev/null || break
    sleep 0.1
done
[ -f "$SOURCE_FILE" ] || { echo "FAIL: Prnsd did not persist the stock RNS source list"; tail -30 "$UPDATER_LOG"; exit 1; }
UPDATER_RESULT="$($PYTHON "$CLIENT" verify-source-file "$SOURCE_FILE" "$STOCK_SOURCE" 2>&1)"
if [[ "$UPDATER_RESULT" != *"BLACKHOLE_UPDATER_OK"* ]]; then
    echo "FAIL: Prnsd's imported source file was not stock-compatible"
    echo "$UPDATER_RESULT"
    exit 1
fi

echo "PASS: stock RNS 1.4.2 fetched Prnsd's blackhole list"
echo "$PUBLISHER_RESULT" | grep "BLACKHOLE_PUBLISHER_OK"
echo "PASS: Prnsd fetched and persisted stock RNS 1.4.2's blackhole list"
echo "$UPDATER_RESULT" | grep "BLACKHOLE_UPDATER_OK"

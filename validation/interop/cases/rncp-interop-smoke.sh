#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/validation/.venv/rns-rpc-1.4.2/bin/python}"
RNCP="$(dirname "$PYTHON")/rncp"
ORACLE="$ROOT/validation/interop/peers/rns_rncp_oracle.py"
BIN="$ROOT/prnsd/target/debug/prnsd"
WORK="$(mktemp -d)"
CONFIG="$WORK/config"
CLIENT_CONFIG="$WORK/client-config"
STOCK_ID="$WORK/stock.rid"
CLIENT_ID="$WORK/client.rid"
PRNS_ID="$WORK/prns.rid"
RNSD_LOG="$WORK/server.log"
CLIENT_LOG="$WORK/client.log"
LISTENER_LOG="$WORK/listener.log"
RNSD_PID=""
CLIENT_PID=""
LISTENER_PID=""
INTERRUPT_PID=""
STOCK_COMMAND_PID=""

cleanup() {
    STATUS=$?
    [ -n "$LISTENER_PID" ] && kill "$LISTENER_PID" 2>/dev/null || true
    [ -n "$LISTENER_PID" ] && wait "$LISTENER_PID" 2>/dev/null || true
    [ -n "$INTERRUPT_PID" ] && kill "$INTERRUPT_PID" 2>/dev/null || true
    [ -n "$INTERRUPT_PID" ] && wait "$INTERRUPT_PID" 2>/dev/null || true
    [ -n "$STOCK_COMMAND_PID" ] && kill "$STOCK_COMMAND_PID" 2>/dev/null || true
    [ -n "$STOCK_COMMAND_PID" ] && wait "$STOCK_COMMAND_PID" 2>/dev/null || true
    [ -n "$CLIENT_PID" ] && kill "$CLIENT_PID" 2>/dev/null || true
    [ -n "$CLIENT_PID" ] && wait "$CLIENT_PID" 2>/dev/null || true
    [ -n "$RNSD_PID" ] && kill "$RNSD_PID" 2>/dev/null || true
    [ -n "$RNSD_PID" ] && wait "$RNSD_PID" 2>/dev/null || true
    if [ "$STATUS" -ne 0 ]; then
        [ -f "$RNSD_LOG" ] && cat "$RNSD_LOG"
        [ -f "$CLIENT_LOG" ] && cat "$CLIENT_LOG"
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

start_stock_command() {
    OUTPUT="$1"
    shift
    "$PYTHON" - "$@" > "$OUTPUT" 2>&1 <<'PY' &
import subprocess
import signal
import sys

process = None


def stop(signum, frame):
    if process is not None:
        process.terminate()
    raise SystemExit(128 + signum)


signal.signal(signal.SIGTERM, stop)
try:
    process = subprocess.Popen(sys.argv[1:])
    result = process.wait(timeout=60)
except subprocess.TimeoutExpired:
    process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
    sys.exit(124)
sys.exit(result)
PY
    STOCK_COMMAND_PID=$!
}

wait_stock_command() {
    set +e
    wait "$STOCK_COMMAND_PID"
    STOCK_COMMAND_STATUS=$?
    set -e
    STOCK_COMMAND_PID=""
    return "$STOCK_COMMAND_STATUS"
}

wait_for_path_request() {
    OUTPUT="$1"
    for _ in $(seq 1 200); do
        if grep -q "Path to" "$OUTPUT"; then
            sleep 1
            return
        fi
        kill -0 "$STOCK_COMMAND_PID" 2>/dev/null || break
        sleep 0.05
    done
    echo "FAIL: stock rncp did not request the fresh listener path"
    cat "$OUTPUT"
    exit 1
}

stop_listener() {
    [ -n "$LISTENER_PID" ] && kill "$LISTENER_PID" 2>/dev/null || true
    [ -n "$LISTENER_PID" ] && wait "$LISTENER_PID" 2>/dev/null || true
    LISTENER_PID=""
}

start_listener() {
    POLICY="$1"
    LISTENER_IDENTITY="$2"
    if [ "$POLICY" = "public" ]; then
        "$BIN" cp --config "$CONFIG" -i "$LISTENER_IDENTITY" -l -n -F -j "$WORK/prns-fetch" -s "$WORK/prns-receive" > "$LISTENER_LOG" 2>&1 &
    else
        "$BIN" cp --config "$CONFIG" -i "$LISTENER_IDENTITY" -l -F -a "$CLIENT_HASH" -s "$WORK/auth-receive" -j "$WORK/prns-fetch" > "$LISTENER_LOG" 2>&1 &
    fi
    LISTENER_PID=$!
    for _ in $(seq 1 100); do
        grep -q "cp listening" "$LISTENER_LOG" && break
        kill -0 "$LISTENER_PID" 2>/dev/null || break
        sleep 0.05
    done
    grep -q "cp listening" "$LISTENER_LOG" || { echo "FAIL: Prns RNCP listener stopped"; cat "$LISTENER_LOG"; exit 1; }
}

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }
[ -x "$RNCP" ] || { echo "FAIL: stock rncp not found at $RNCP"; exit 1; }

PORTS="$($PYTHON - <<'PY'
import socket
sockets = []
ports = []
for _ in range(5):
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
CLIENT_BUS_PORT="$4"
CLIENT_CONTROL_PORT="$5"
STOCK_DESTINATION="$($PYTHON "$ORACLE" prepare "$CONFIG" "$CLIENT_CONFIG" "$BUS_PORT" "$CONTROL_PORT" "$NETWORK_PORT" "$CLIENT_BUS_PORT" "$CLIENT_CONTROL_PORT" "$STOCK_ID" "$CLIENT_ID")"

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
"$PYTHON" "$ORACLE" hold "$CLIENT_CONFIG" > "$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!
for _ in $(seq 1 100); do
    grep -q "RNCP_CLIENT_READY" "$CLIENT_LOG" && break
    kill -0 "$CLIENT_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNCP_CLIENT_READY" "$CLIENT_LOG" || { echo "FAIL: stock RNCP client instance stopped"; cat "$CLIENT_LOG"; exit 1; }

"$PYTHON" - "$WORK" <<'PY'
import hashlib
import os
import pathlib
import sys

import RNS

work = pathlib.Path(sys.argv[1])
work.joinpath("prns-send.bin").write_bytes(b"prns-to-stock\n" * 12000)
work.joinpath("stock-send.bin").write_bytes(b"stock-to-prns\n" * 12000)
work.joinpath("prns-compressed.bin").write_bytes(b"prns-compressed-resource\n" * 12000)
work.joinpath("stock-compressed.bin").write_bytes(b"stock-compressed-resource\n" * 12000)
work.joinpath("stock-fetch/stock.txt").write_bytes(b"served-by-stock\n" * 12000)
work.joinpath("prns-fetch/prns.txt").write_bytes(b"served-by-prns\n" * 12000)
work.joinpath("interrupt-prns.bin").write_bytes(os.urandom(32 * 1024 * 1024))
work.joinpath("cancel-stock.bin").write_bytes(os.urandom(32 * 1024 * 1024))
segment_crossing_size = RNS.Resource.MAX_EFFICIENT_SIZE + 4096
for name, seed in (("prns-segmented.bin", b"prns"), ("stock-segmented.bin", b"stock")):
    blocks = []
    generated = 0
    counter = 0
    while generated < segment_crossing_size:
        block = hashlib.sha256(seed + counter.to_bytes(8, "big")).digest()
        blocks.append(block)
        generated += len(block)
        counter += 1
    work.joinpath(name).write_bytes(b"".join(blocks)[:segment_crossing_size])
for size in (1, 464, 465):
    work.joinpath(f"boundary-{size}.bin").write_bytes(bytes((index * 37) & 0xff for index in range(size)))
PY

"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -C "$WORK/interrupt-prns.bin" "$STOCK_DESTINATION" > "$WORK/interrupted-prns.out" 2>&1 &
INTERRUPT_PID=$!
INTERRUPT_STARTED=""
for _ in $(seq 1 200); do
    grep -q "Transferring file" "$WORK/interrupted-prns.out" && { INTERRUPT_STARTED=1; break; }
    kill -0 "$INTERRUPT_PID" 2>/dev/null || break
    sleep 0.05
done
[ -n "$INTERRUPT_STARTED" ] || { echo "FAIL: Prns interruption transfer never started"; cat "$WORK/interrupted-prns.out"; exit 1; }
kill "$INTERRUPT_PID"
set +e
wait "$INTERRUPT_PID"
INTERRUPTED_STATUS=$?
set -e
INTERRUPT_PID=""
[ "$INTERRUPTED_STATUS" -ne 0 ] || { echo "FAIL: interrupted Prns sender reported success"; exit 1; }
sleep 1
[ ! -f "$WORK/stock-receive/interrupt-prns.bin" ] || { echo "FAIL: interrupted Prns bytes were published by stock RNCP"; exit 1; }

"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -S -P "$WORK/prns-send.bin" "$STOCK_DESTINATION"
for _ in $(seq 1 100); do
    [ -f "$WORK/stock-receive/prns-send.bin" ] && break
    sleep 0.1
done
cmp "$WORK/prns-send.bin" "$WORK/stock-receive/prns-send.bin" || { echo "FAIL: stock rncp did not receive Prns bytes"; cat "$LISTENER_LOG"; exit 1; }
"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -S "$WORK/prns-compressed.bin" "$STOCK_DESTINATION"
for _ in $(seq 1 100); do
    [ -f "$WORK/stock-receive/prns-compressed.bin" ] && break
    sleep 0.1
done
cmp "$WORK/prns-compressed.bin" "$WORK/stock-receive/prns-compressed.bin" || { echo "FAIL: stock rncp did not reconstruct the compressed Prns bytes"; cat "$RNSD_LOG"; exit 1; }
grep -q "RNCP_COMPRESSED_RECEIVED name=prns-compressed.bin" "$RNSD_LOG" || { echo "FAIL: stock RNS did not observe compressed transport from Prns"; cat "$RNSD_LOG"; exit 1; }
"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -S -P -C "$WORK/prns-segmented.bin" "$STOCK_DESTINATION"
for _ in $(seq 1 200); do
    [ -f "$WORK/stock-receive/prns-segmented.bin" ] && break
    sleep 0.1
done
cmp "$WORK/prns-segmented.bin" "$WORK/stock-receive/prns-segmented.bin" || { echo "FAIL: stock rncp did not receive the completed segmented Prns resource"; cat "$RNSD_LOG"; exit 1; }
grep -q "RNCP_SEGMENTED_RECEIVED name=prns-segmented.bin segments=" "$RNSD_LOG" || { echo "FAIL: stock RNS did not observe multiple segments from Prns"; cat "$RNSD_LOG"; exit 1; }

PRNS_DESTINATION="$($BIN cp --config "$CONFIG" -i "$PRNS_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$PRNS_DESTINATION" ] || { echo "FAIL: Prns listener destination unavailable"; exit 1; }
"$PYTHON" "$ORACLE" cancel-send "$CLIENT_CONFIG" "$CLIENT_ID" "$PRNS_DESTINATION" "$WORK/cancel-stock.bin" "$WORK/boundary-1.bin" "$WORK/boundary-464.bin" "$WORK/boundary-465.bin" "$WORK/stock-send.bin" "$WORK/stock-compressed.bin" "$WORK/stock-segmented.bin" > "$WORK/cancel-result.out" 2>&1 &
STOCK_COMMAND_PID=$!
for _ in $(seq 1 200); do
    grep -q "RNCP_CANCEL_PATH_REQUESTED" "$WORK/cancel-result.out" && break
    kill -0 "$STOCK_COMMAND_PID" 2>/dev/null || break
    sleep 0.05
done
grep -q "RNCP_CANCEL_PATH_REQUESTED" "$WORK/cancel-result.out" || { echo "FAIL: stock cancellation oracle did not request the listener path"; cat "$WORK/cancel-result.out"; exit 1; }
sleep 1
start_listener public "$PRNS_ID"
wait_stock_command || { echo "FAIL: stock resource cancellation did not settle"; cat "$WORK/cancel-result.out"; exit 1; }
RESULT="$(cat "$WORK/cancel-result.out")"
[[ "$RESULT" == *"RNCP_CANCEL_OK"* ]] || { echo "FAIL: stock resource cancellation did not settle"; echo "$RESULT"; exit 1; }
[[ "$RESULT" == *"segmented_recoveries=1"* ]] || { echo "FAIL: stock RNS did not complete a multi-segment recovery into Prns"; echo "$RESULT"; exit 1; }
[[ "$RESULT" == *"compressed_recoveries=1"* ]] || { echo "FAIL: stock RNS did not send a compressed Resource into Prns"; echo "$RESULT"; exit 1; }
for _ in $(seq 1 100); do
    STAGING="$(find "$WORK/prns-receive" -maxdepth 1 -name '.rncp.*.staging' -print -quit)"
    [ -z "$STAGING" ] && break
    sleep 0.1
done
[ ! -f "$WORK/prns-receive/cancel-stock.bin" ] || { echo "FAIL: cancelled stock bytes were published"; exit 1; }
[ -z "$(find "$WORK/prns-receive" -maxdepth 1 -name '.rncp.*.staging' -print -quit)" ] || { echo "FAIL: cancelled stock transfer left staging state"; exit 1; }
for name in boundary-1.bin boundary-464.bin boundary-465.bin stock-send.bin stock-compressed.bin stock-segmented.bin; do
    for _ in $(seq 1 100); do
        [ -f "$WORK/prns-receive/$name" ] && break
        sleep 0.1
    done
    cmp "$WORK/$name" "$WORK/prns-receive/$name" || { echo "FAIL: RNCP recovery file $name did not round-trip"; exit 1; }
done
"$BIN" cp --config "$CONFIG" -i "$PRNS_ID" -S -P -f -s "$WORK/prns-fetched" stock.txt "$STOCK_DESTINATION"
cmp "$WORK/stock-fetch/stock.txt" "$WORK/prns-fetched/stock.txt" || { echo "FAIL: Prns did not fetch stock rncp bytes"; cat "$LISTENER_LOG"; exit 1; }

stop_listener
PUBLIC_FETCH_ID="$WORK/public-fetch.rid"
PUBLIC_FETCH_DESTINATION="$($BIN cp --config "$CONFIG" -i "$PUBLIC_FETCH_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$PUBLIC_FETCH_DESTINATION" ] || { echo "FAIL: public fetch listener destination unavailable"; exit 1; }
start_stock_command "$WORK/public-fetch.out" "$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -f -s "$WORK/stock-fetched" prns.txt "$PUBLIC_FETCH_DESTINATION"
wait_for_path_request "$WORK/public-fetch.out"
start_listener public "$PUBLIC_FETCH_ID"
wait_stock_command || { echo "FAIL: stock rncp public fetch failed"; cat "$WORK/public-fetch.out"; exit 1; }
cmp "$WORK/prns-fetch/prns.txt" "$WORK/stock-fetched/prns.txt" || { echo "FAIL: stock rncp did not fetch Prns bytes"; cat "$LISTENER_LOG"; exit 1; }
stop_listener

CLIENT_HASH="$($PYTHON "$ORACLE" identity-hash "$CLIENT_ID")"
AUTH_SEND_ID="$WORK/auth-send.rid"
AUTH_SEND_DESTINATION="$($BIN cp --config "$CONFIG" -i "$AUTH_SEND_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$AUTH_SEND_DESTINATION" ] || { echo "FAIL: authenticated send listener destination unavailable"; exit 1; }
start_stock_command "$WORK/auth-send.out" "$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" "$WORK/stock-send.bin" "$AUTH_SEND_DESTINATION"
wait_for_path_request "$WORK/auth-send.out"
start_listener authenticated "$AUTH_SEND_ID"
wait_stock_command || { echo "FAIL: authenticated stock sender was rejected"; cat "$WORK/auth-send.out"; cat "$LISTENER_LOG"; exit 1; }
for _ in $(seq 1 100); do
    [ -f "$WORK/auth-receive/stock-send.bin" ] && break
    sleep 0.1
done
cmp "$WORK/stock-send.bin" "$WORK/auth-receive/stock-send.bin" || { echo "FAIL: authenticated stock sender was rejected"; cat "$LISTENER_LOG"; exit 1; }
stop_listener
AUTH_FETCH_ID="$WORK/auth-fetch.rid"
AUTH_FETCH_DESTINATION="$($BIN cp --config "$CONFIG" -i "$AUTH_FETCH_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$AUTH_FETCH_DESTINATION" ] || { echo "FAIL: authenticated fetch listener destination unavailable"; exit 1; }
start_stock_command "$WORK/auth-fetch.out" "$RNCP" --config "$CLIENT_CONFIG" -i "$CLIENT_ID" -f -s "$WORK/auth-fetched" prns.txt "$AUTH_FETCH_DESTINATION"
wait_for_path_request "$WORK/auth-fetch.out"
start_listener authenticated "$AUTH_FETCH_ID"
wait_stock_command || { echo "FAIL: authenticated stock fetch was rejected"; cat "$WORK/auth-fetch.out"; cat "$LISTENER_LOG"; exit 1; }
cmp "$WORK/prns-fetch/prns.txt" "$WORK/auth-fetched/prns.txt" || { echo "FAIL: authenticated stock fetch was rejected"; cat "$LISTENER_LOG"; exit 1; }
stop_listener
DENIED_ID="$WORK/denied.rid"
DENIED_DESTINATION="$($BIN cp --config "$CONFIG" -i "$DENIED_ID" -p | sed -n 's/^Listening on : <\([0-9a-f]*\)>$/\1/p')"
[ -n "$DENIED_DESTINATION" ] || { echo "FAIL: denied listener destination unavailable"; exit 1; }
start_stock_command "$WORK/denied.out" "$RNCP" --config "$CLIENT_CONFIG" -i "$STOCK_ID" -w 5 "$WORK/prns-send.bin" "$DENIED_DESTINATION"
wait_for_path_request "$WORK/denied.out"
start_listener authenticated "$DENIED_ID"
if wait_stock_command; then
    echo "FAIL: unlisted stock sender was accepted"
    cat "$WORK/denied.out"
    exit 1
fi
[ ! -f "$WORK/auth-receive/prns-send.bin" ] || { echo "FAIL: unlisted stock bytes were published"; exit 1; }
stop_listener

echo "PASS: Prnsd cp rejects partial publication, settles cancellation, and exchanges exact compressed, boundary, bulk, and completed multi-segment files with stock RNS 1.4.2 rncp"

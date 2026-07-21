#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/benchmarks/reference/.rpc-venv/bin/python}"
SERVER="$ROOT/prns-core/tests/interop/rns_rnpath_server.py"
WORK="$(mktemp -d)"
CONFIG="$WORK/config"
PEER_CONFIG="$WORK/peer-config"
MANAGEMENT_IDENTITY="$WORK/management_identity"
SERVER_LOG="$WORK/server.log"
PEER_LOG="$WORK/peer.log"
SERVER_PID=""
PEER_PID=""
BLACKHOLE_HASH="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
RATE_HASH="33333333333333333333333333333333"

cleanup() {
    [ -n "$PEER_PID" ] && kill "$PEER_PID" 2>/dev/null
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
    [ -n "$PEER_PID" ] && wait "$PEER_PID" 2>/dev/null
    [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null
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
)" || { echo "FAIL: could not allocate loopback ports"; exit 1; }
set -- $PORTS
BUS_PORT="${1:-}"
CONTROL_PORT="${2:-}"
NETWORK_PORT="${3:-}"
[ -n "$BUS_PORT" ] && [ -n "$CONTROL_PORT" ] && [ -n "$NETWORK_PORT" ] || { echo "FAIL: empty oracle ports"; exit 1; }

"$PYTHON" "$SERVER" prepare "$CONFIG" "$PEER_CONFIG" "$BUS_PORT" "$CONTROL_PORT" "$NETWORK_PORT" "$MANAGEMENT_IDENTITY"
( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }
"$PYTHON" "$SERVER" serve "$CONFIG" > "$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 100); do
    grep -q "RNPATH_SERVER_READY" "$SERVER_LOG" && break
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNPATH_SERVER_READY" "$SERVER_LOG" || {
    echo "FAIL: stock RNS rnpath server never became ready"
    cat "$SERVER_LOG"
    exit 1
}

"$PYTHON" "$SERVER" peer "$PEER_CONFIG" > "$PEER_LOG" 2>&1 &
PEER_PID=$!
for _ in $(seq 1 100); do
    grep -q "RNPATH_PEER_READY" "$PEER_LOG" && break
    kill -0 "$PEER_PID" 2>/dev/null || break
    sleep 0.1
done
grep -q "RNPATH_PEER_READY" "$PEER_LOG" || {
    echo "FAIL: stock RNS rnpath peer never became ready"
    cat "$PEER_LOG"
    exit 1
}
PEER_HASH="$(sed -n 's/^RNPATH_PEER_READY //p' "$PEER_LOG" | head -n 1)"
[ -n "$PEER_HASH" ] || { echo "FAIL: oracle peer hash was unavailable"; exit 1; }

PATH_RESULT=""
for _ in $(seq 1 100); do
    PATH_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -t -j 2>&1)" && [[ "$PATH_RESULT" == *"$PEER_HASH"* ]] && break
    sleep 0.1
done
echo "$PATH_RESULT" | "$PYTHON" -c 'import json, sys; rows=json.load(sys.stdin); expected=sys.argv[1]; assert any(row["hash"] == expected and row["hops"] >= 1 and row["interface"] for row in rows)' "$PEER_HASH" || {
    echo "FAIL: Prnsd did not decode the stock local path table"
    echo "$PATH_RESULT"
    cat "$SERVER_LOG"
    cat "$PEER_LOG"
    exit 1
}
VIA_HASH="$(echo "$PATH_RESULT" | "$PYTHON" -c 'import json, sys; rows=json.load(sys.stdin); expected=sys.argv[1]; print(next(row["via"] for row in rows if row["hash"] == expected))' "$PEER_HASH")"
[ -n "$VIA_HASH" ] || { echo "FAIL: oracle peer next hop was unavailable"; exit 1; }

RATE_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -r -j 2>&1)" || {
    echo "FAIL: Prnsd could not query stock announce rates"
    echo "$RATE_RESULT"
    exit 1
}
echo "$RATE_RESULT" | "$PYTHON" -c 'import json, sys; rows=json.load(sys.stdin); expected=sys.argv[1]; row=next(row for row in rows if row["hash"] == expected); assert row["rate_violations"] == 3 and len(row["timestamps"]) == 2' "$RATE_HASH" || {
    echo "FAIL: Prnsd did not decode the stock announce-rate table"
    echo "$RATE_RESULT"
    exit 1
}

TRANSPORT_HASH="$($PYTHON "$SERVER" identity-hash "$CONFIG/storage/transport_identity")"
[ -n "$TRANSPORT_HASH" ] || { echo "FAIL: stock transport identity was unavailable"; exit 1; }
REMOTE_PATH_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -t -j -R "$TRANSPORT_HASH" -i "$MANAGEMENT_IDENTITY" 2>&1)" || {
    echo "FAIL: Prnsd could not query the stock remote path table"
    echo "$REMOTE_PATH_RESULT"
    exit 1
}
echo "$REMOTE_PATH_RESULT" | "$PYTHON" -c 'import json, sys; rows=json.load(sys.stdin); expected=sys.argv[1]; assert any(row["hash"] == expected for row in rows)' "$PEER_HASH" || {
    echo "FAIL: Prnsd did not decode the stock remote path table"
    echo "$REMOTE_PATH_RESULT"
    exit 1
}
REMOTE_RATE_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -r -j -R "$TRANSPORT_HASH" -i "$MANAGEMENT_IDENTITY" 2>&1)" || {
    echo "FAIL: Prnsd could not query the stock remote rate table"
    echo "$REMOTE_RATE_RESULT"
    exit 1
}
echo "$REMOTE_RATE_RESULT" | "$PYTHON" -c 'import json, sys; rows=json.load(sys.stdin); expected=sys.argv[1]; assert any(row["hash"] == expected for row in rows)' "$RATE_HASH" || {
    echo "FAIL: Prnsd did not decode the stock remote rate table"
    echo "$REMOTE_RATE_RESULT"
    exit 1
}

"$ROOT/prnsd/target/debug/prnsd" path --config "$CONFIG" -B --duration 1 --reason oracle "$BLACKHOLE_HASH" > "$WORK/blackhole-add.out" || {
    echo "FAIL: Prnsd could not add a stock local blackhole"
    cat "$WORK/blackhole-add.out"
    exit 1
}
BLACKHOLE_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -b 2>&1)" || {
    echo "FAIL: Prnsd could not list stock local blackholes"
    echo "$BLACKHOLE_RESULT"
    exit 1
}
[[ "$BLACKHOLE_RESULT" == *"<$BLACKHOLE_HASH> blackholed for "*" (oracle)"* ]] || {
    echo "FAIL: Prnsd rendered unexpected local blackhole data"
    echo "$BLACKHOLE_RESULT"
    exit 1
}
PUBLISHED_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -p "$TRANSPORT_HASH" 2>&1)" || {
    echo "FAIL: Prnsd could not query the stock published blackhole list"
    echo "$PUBLISHED_RESULT"
    exit 1
}
[[ "$PUBLISHED_RESULT" == *"<$BLACKHOLE_HASH> blackholed for "*" (oracle)"* ]] || {
    echo "FAIL: Prnsd rendered unexpected published blackhole data"
    echo "$PUBLISHED_RESULT"
    exit 1
}
"$ROOT/prnsd/target/debug/prnsd" path --config "$CONFIG" -U "$BLACKHOLE_HASH" > "$WORK/blackhole-remove.out" || {
    echo "FAIL: Prnsd could not remove a stock local blackhole"
    cat "$WORK/blackhole-remove.out"
    exit 1
}

REQUEST_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -w 10 "$PEER_HASH" 2>&1)" || {
    echo "FAIL: Prnsd could not request the stock peer path"
    echo "$REQUEST_RESULT"
    exit 1
}
[[ "$REQUEST_RESULT" == *"Path found, destination <$PEER_HASH>"* ]] || {
    echo "FAIL: Prnsd rendered unexpected path request data"
    echo "$REQUEST_RESULT"
    exit 1
}
"$ROOT/prnsd/target/debug/prnsd" path --config "$CONFIG" -x "$VIA_HASH" > "$WORK/drop-via.out" || {
    echo "FAIL: Prnsd could not drop stock paths through a transport"
    cat "$WORK/drop-via.out"
    exit 1
}
for _ in $(seq 1 100); do
    PATH_RESULT="$($ROOT/prnsd/target/debug/prnsd path --config "$CONFIG" -t -j 2>&1)" && [[ "$PATH_RESULT" == *"$PEER_HASH"* ]] && break
    sleep 0.1
done
[[ "$PATH_RESULT" == *"$PEER_HASH"* ]] || {
    echo "FAIL: stock peer route did not return after dropping its transport"
    echo "$PATH_RESULT"
    exit 1
}
"$ROOT/prnsd/target/debug/prnsd" path --config "$CONFIG" -d "$PEER_HASH" > "$WORK/drop.out" || {
    echo "FAIL: Prnsd could not drop a stock path"
    cat "$WORK/drop.out"
    exit 1
}
"$ROOT/prnsd/target/debug/prnsd" path --config "$CONFIG" -D > "$WORK/drop-announces.out" || {
    echo "FAIL: Prnsd could not drop stock announce queues"
    cat "$WORK/drop-announces.out"
    exit 1
}

echo "PASS: Prnsd path queried and mutated the stock RNS 1.4.0 utility surfaces"

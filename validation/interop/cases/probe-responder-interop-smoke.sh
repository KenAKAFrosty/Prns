#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$ROOT/validation/interop/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
PYTHON="${RPC_SMOKE_PYTHON:?RPC_SMOKE_PYTHON is set by validation/run.py}"
CLIENT="$ROOT/validation/interop/peers/rns_probe_client.py"
WORK="$(mktemp -d)"
SERVER_CONFIG="$WORK/server"
CLIENT_CONFIG="$WORK/client"
DAEMON_PID=""

cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    [ -n "$DAEMON_PID" ] && wait "$DAEMON_PID" 2>/dev/null
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }

PORT="$($PYTHON -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
$PYTHON "$CLIENT" prepare "$SERVER_CONFIG" "$CLIENT_CONFIG" "$PORT"

( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }
"$PRNSD" run --log-format json --config "$SERVER_CONFIG" &
DAEMON_PID=$!

for _ in $(seq 1 100); do
    "$PYTHON" -c 'import socket, sys; sock = socket.socket(); sock.settimeout(0.1); sys.exit(sock.connect_ex(("127.0.0.1", int(sys.argv[1]))))' "$PORT" && break
    kill -0 "$DAEMON_PID" 2>/dev/null || break
    sleep 0.1
done
"$PYTHON" -c 'import socket, sys; sock = socket.socket(); sock.settimeout(0.1); sys.exit(sock.connect_ex(("127.0.0.1", int(sys.argv[1]))))' "$PORT" || { echo "FAIL: prnsd listener never became ready"; exit 1; }

TRANSPORT_HASH="$($PYTHON "$CLIENT" identity-hash "$SERVER_CONFIG/storage/transport_identity")"
[ -n "$TRANSPORT_HASH" ] || { echo "FAIL: transport identity was not readable by RNS"; exit 1; }

RESULT="$($PYTHON "$CLIENT" probe "$CLIENT_CONFIG" "$TRANSPORT_HASH" 2>&1)"
if [[ "$RESULT" == *"PROBE_RESPONDER_OK"* ]]; then
    echo "PASS: stock RNS received Prnsd's delivery proof from rnstransport.probe"
    echo "$RESULT" | grep "PROBE_RESPONDER_OK"
    exit 0
fi

echo "FAIL: stock RNS probe did not receive a valid delivery proof"
echo "$RESULT"
exit 1

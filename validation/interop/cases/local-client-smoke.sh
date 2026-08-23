#!/usr/bin/env bash
# Real-RNS inverse-parity smoke: a stock RNS shared instance runs first, then prnsd detects
# it and joins as an honorable client over its bus — standing up none of its own interfaces, the way
# a stock RNS app defers to a running instance. The mirror of local_interop_smoke.py (which proves
# Prns-as-server). The lane uses explicit ephemeral TCP ports so a developer's real default shared
# instance can keep running alongside validation.
#
# The Python interpreter is $SMOKE_PYTHON if set (CI points it at a uv-built venv with the pinned rns
# from validation/oracles/requirements.txt), otherwise the local reference venv. Needs a free
# loopback ports. Prints PASS or FAIL and exits accordingly.
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$ROOT/validation/interop/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
VENV_PY="${SMOKE_PYTHON:?SMOKE_PYTHON is set by validation/run.py}"
STOCK_DIR="$(mktemp -d)"
PRNS_DIR="$(mktemp -d)"
STOCK_LOG="$(mktemp)"
PRNSD_LOG="$(mktemp)"
STATUS_LOG="$(mktemp)"
STOCK_PID=""
PRNSD_PID=""
RPC_KEY="${PRNS_RPC_KEY:-$(printf '5a%.0s' $(seq 1 32))}"

cleanup() {
    [ -n "$PRNSD_PID" ] && kill "$PRNSD_PID" 2>/dev/null
    [ -n "$STOCK_PID" ] && kill "$STOCK_PID" 2>/dev/null
    wait "$PRNSD_PID" "$STOCK_PID" 2>/dev/null
    rm -rf "$STOCK_DIR" "$PRNS_DIR"
}
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }

read -r BUS_PORT CONTROL_PORT STOCK_LISTENER_PORT PRNS_LISTENER_PORT < <("$VENV_PY" -c '
import socket
sockets = [socket.socket() for _ in range(4)]
for sock in sockets: sock.bind(("127.0.0.1", 0))
print(*(sock.getsockname()[1] for sock in sockets))
for sock in sockets: sock.close()
')

# The stock RNS instance owns an isolated TCP listener and the bus. Avoiding
# AutoInterface keeps the inverse smoke independent of host multicast state.
cat > "$STOCK_DIR/config" <<EOF
[reticulum]
  enable_transport = No
  share_instance = Yes
  shared_instance_type = tcp
  shared_instance_port = $BUS_PORT
  instance_control_port = $CONTROL_PORT
  rpc_key = $RPC_KEY
[interfaces]
  [[Stock Listener]]
    type = TCPServerInterface
    interface_enabled = Yes
    listen_ip = 127.0.0.1
    listen_port = $STOCK_LISTENER_PORT
EOF

# prnsd carries a TCP server interface it must NOT stand up while it is a client.
cat > "$PRNS_DIR/config" <<EOF
[reticulum]
  enable_transport = No
  share_instance = Yes
  shared_instance_type = tcp
  shared_instance_port = $BUS_PORT
  instance_control_port = $CONTROL_PORT
  rpc_key = $RPC_KEY
[interfaces]
  [[Listener]]
    type = TCPServerInterface
    interface_enabled = Yes
    listen_ip = 127.0.0.1
    listen_port = $PRNS_LISTENER_PORT
EOF

echo "building prnsd..."
( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }

echo "starting the stock RNS shared instance..."
"$VENV_PY" -c "import RNS, time; RNS.Reticulum(configdir='$STOCK_DIR'); print('STOCK_INSTANCE_UP', flush=True); time.sleep(30)" > "$STOCK_LOG" 2>&1 &
STOCK_PID=$!
for _ in $(seq 1 80); do grep -q "STOCK_INSTANCE_UP" "$STOCK_LOG" && break; sleep 0.25; done
grep -q "STOCK_INSTANCE_UP" "$STOCK_LOG" || { echo "FAIL: the stock RNS instance never came up"; tail -20 "$STOCK_LOG"; exit 1; }
sleep 0.5
echo "stock instance up; running prnsd against the same bus..."

RUST_LOG=info "$PRNSD" run --config "$PRNS_DIR" > "$PRNSD_LOG" 2>&1 &
PRNSD_PID=$!
for _ in $(seq 1 60); do grep -q 'event="daemon_ready"' "$PRNSD_LOG" && break; sleep 0.2; done
sleep 0.3

if grep -q 'event="shared_instance_joined"' "$PRNSD_LOG" && ! grep -q 'event="interface_started"' "$PRNSD_LOG" \
    && "$PRNSD" status --config "$PRNS_DIR" --json > "$STATUS_LOG" 2>&1 \
    && grep -q '"interfaces"' "$STATUS_LOG"; then
    echo "PASS: prnsd joined stock RNS as a client, started no interfaces, and decoded its control-RPC status"
    grep -E 'event="shared_instance_joined"|event="shared_instance_started"' "$PRNSD_LOG" | head -1
    exit 0
fi

echo "FAIL: prnsd did not join the stock instance as a client"
echo "--- stock log ---"; cat "$STOCK_LOG"
echo "--- prnsd log ---"; cat "$PRNSD_LOG"
echo "--- status log ---"; cat "$STATUS_LOG"
exit 1

#!/usr/bin/env bash
# Real-RNS inverse-parity smoke: a stock RNS shared instance runs first, then prnsd detects
# it and joins as an honorable client over its bus — standing up none of its own interfaces, the way
# a stock RNS app defers to a running instance. The mirror of local-interop-smoke.sh (which proves
# Prns-as-server). On Linux this exercises the abstract AF_UNIX bus a default-config instance prefers;
# on macOS, the TCP bus.
#
# The Python interpreter is $SMOKE_PYTHON if set (CI points it at a uv-built venv with the pinned rns
# from benchmarks/reference/requirements.txt), otherwise the local reference venv. Needs a free
# loopback 37428 (the RNS default). Prints PASS or FAIL and exits accordingly.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
source "$ROOT/scripts/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/benchmarks/reference/.venv/bin/python}"
STOCK_DIR="$(mktemp -d)"
PRNS_DIR="$(mktemp -d)"
STOCK_LOG="$(mktemp)"
PRNSD_LOG="$(mktemp)"
STOCK_PID=""
PRNSD_PID=""

cleanup() {
    [ -n "$PRNSD_PID" ] && kill "$PRNSD_PID" 2>/dev/null
    [ -n "$STOCK_PID" ] && kill "$STOCK_PID" 2>/dev/null
    wait "$PRNSD_PID" "$STOCK_PID" 2>/dev/null
    rm -rf "$STOCK_DIR" "$PRNS_DIR"
}
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }

# The stock RNS instance owns a Default AutoInterface and the bus.
cat > "$STOCK_DIR/config" <<EOF
[reticulum]
  enable_transport = No
  share_instance = Yes
[interfaces]
  [[Default Interface]]
    type = AutoInterface
    interface_enabled = Yes
EOF

# prnsd carries a TCP server interface it must NOT stand up while it is a client.
cat > "$PRNS_DIR/config" <<EOF
[reticulum]
  enable_transport = No
  share_instance = Yes
[interfaces]
  [[Listener]]
    type = TCPServerInterface
    interface_enabled = Yes
    listen_ip = 127.0.0.1
    listen_port = 45981
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

"$PRNSD" run --config "$PRNS_DIR" > "$PRNSD_LOG" 2>&1 &
PRNSD_PID=$!
for _ in $(seq 1 60); do grep -q 'event="daemon_ready"' "$PRNSD_LOG" && break; sleep 0.2; done
sleep 0.3

if grep -q 'event="shared_instance_joined"' "$PRNSD_LOG" && ! grep -q 'event="interface_started"' "$PRNSD_LOG"; then
    echo "PASS: prnsd detected the stock RNS instance and joined as a client, standing up none of its own interfaces"
    grep -E 'event="shared_instance_joined"|event="shared_instance_started"' "$PRNSD_LOG" | head -1
    exit 0
fi

echo "FAIL: prnsd did not join the stock instance as a client"
echo "--- stock log ---"; cat "$STOCK_LOG"
echo "--- prnsd log ---"; cat "$PRNSD_LOG"
exit 1

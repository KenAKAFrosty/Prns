#!/usr/bin/env bash
set -eu

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$ROOT/validation/interop/lib/cargo-artifacts.sh"
PRNSD="$(cargo_debug_binary "$ROOT/prnsd/Cargo.toml" prnsd)"
PYTHON="${RPC_SMOKE_PYTHON:-$ROOT/validation/.venv/rns-rpc-1.4.2/bin/python}"
DEVICE="$ROOT/validation/interop/peers/rns_rnode_tcp_device.py"
WORK="$(mktemp -d)"
PRNS_PID=""
DEVICE_PID=""

cleanup() {
    if [ -n "$PRNS_PID" ]; then
        kill "$PRNS_PID" 2>/dev/null || true
        wait "$PRNS_PID" 2>/dev/null || true
    fi
    if [ -n "$DEVICE_PID" ]; then
        kill "$DEVICE_PID" 2>/dev/null || true
        wait "$DEVICE_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

[ -x "$PYTHON" ] || { echo "FAIL: reference venv python not found at $PYTHON"; exit 1; }

( cd "$ROOT/prnsd" && cargo build --quiet ) || { echo "FAIL: prnsd build"; exit 1; }

CONFIG="$WORK/prns"
READY="$WORK/device-ready"
RESULT="$WORK/device-result"
"$PYTHON" "$DEVICE" prepare "$CONFIG"
"$PYTHON" "$DEVICE" serve "$READY" >"$RESULT" 2>&1 &
DEVICE_PID=$!

for _ in $(seq 1 100); do
    [ -f "$READY" ] && break
    kill -0 "$DEVICE_PID" 2>/dev/null || break
    sleep 0.1
done
[ -f "$READY" ] || { echo "FAIL: reference RNode TCP device did not listen"; cat "$RESULT"; exit 1; }

"$PRNSD" run --log-format json --config "$CONFIG" >/dev/null 2>&1 &
PRNS_PID=$!

for _ in $(seq 1 600); do
    kill -0 "$DEVICE_PID" 2>/dev/null || break
    kill -0 "$PRNS_PID" 2>/dev/null || break
    sleep 0.1
done

if kill -0 "$DEVICE_PID" 2>/dev/null; then
    echo "FAIL: Prnsd did not complete RNode TCP bring-up"
    cat "$RESULT"
    exit 1
fi
wait "$DEVICE_PID" || { DEVICE_PID=""; echo "FAIL: reference RNode TCP device rejected Prnsd"; cat "$RESULT"; exit 1; }
DEVICE_PID=""
grep -q "RNODE_TCP_DEVICE_OK" "$RESULT" || { echo "FAIL: missing RNode TCP success marker"; cat "$RESULT"; exit 1; }

echo "PASS: Prnsd rejected hostile RNode bring-up sequences and recovered against a split-frame RNS 1.4.2 oracle"
grep "RNODE_TCP_DEVICE_OK" "$RESULT"

exit 0

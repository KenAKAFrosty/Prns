#!/usr/bin/env bash
# Real-RNS interop smoke test for the local shared-instance interface.
#
# Stands up the Prns LocalServer daemon (the `local_shared_instance` example), then drives a stock
# RNS.Reticulum client from the reference venv that connects to it as a shared-instance client and
# announces a destination. Asserts the daemon heard that exact destination over a LocalClient
# interface at the discounted hop (hops=0) — a genuine RNS-1.3.1-on-the-wire interop check.
#
# The Python interpreter is $SMOKE_PYTHON if set (CI points it at a uv-built venv with the pinned
# rns==1.3.1 from benchmarks/reference/requirements.txt), otherwise the local reference venv. Needs a
# free loopback 37428. Prints PASS or FAIL and exits accordingly.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/benchmarks/reference/.venv/bin/python}"
CLIENT="$ROOT/personal-rns/tests/interop/rns_shared_instance_client.py"
DAEMON_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
DAEMON_PID=""

cleanup() { [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null; }
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }

echo "building the daemon example..."
( cd "$ROOT" && cargo build --quiet --example local_shared_instance --features local ) \
    || { echo "FAIL: daemon build"; exit 1; }

"$ROOT/target/debug/examples/local_shared_instance" > "$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!

for _ in $(seq 1 50); do grep -q "READY" "$DAEMON_LOG" && break; sleep 0.2; done
grep -q "READY" "$DAEMON_LOG" || { echo "FAIL: daemon never became READY"; cat "$DAEMON_LOG"; exit 1; }
echo "daemon listening; running the RNS client..."

"$VENV_PY" "$CLIENT" > "$CLIENT_LOG" 2>/dev/null
DEST="$(grep -o 'ANNOUNCED dest=[0-9a-f]*' "$CLIENT_LOG" | head -1 | cut -d= -f2)"
[ -n "$DEST" ] || { echo "FAIL: the RNS client never announced"; echo "--- client log ---"; tail -20 "$CLIENT_LOG"; exit 1; }
echo "RNS client announced dest=$DEST"

sleep 0.5
if grep -q "HEARD dest=$DEST hops=0 kind=Some(LocalClient)" "$DAEMON_LOG"; then
    echo "PASS: the Prns LocalServer heard a real RNS client's announce"
    grep "HEARD dest=$DEST" "$DAEMON_LOG" | head -1
    exit 0
fi

echo "FAIL: daemon did not hear dest=$DEST as a discounted LocalClient announce"
echo "--- daemon log (tail) ---"; tail -25 "$DAEMON_LOG"
exit 1

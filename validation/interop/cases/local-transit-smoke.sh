#!/usr/bin/env bash
# Real-RNS transit smoke test for the local shared-instance bridge.
#
# Proves that two real RNS apps separated by the Prns shared instance can establish *links* through it
# and deliver messages both ways — the path LXMF uses for direct messages, not just announce
# propagation. Topology: a stock RNS TCP peer <-> the Prns bridge daemon (LocalServer + TCP client, a
# real transport node) <-> a stock RNS local client. Each hosts a destination, announces it across the
# bridge, links to the other, and sends over the link. The bridge must transport link requests, proofs,
# and link data both ways, including inbound to the local client's own destination. Asserts both ends
# RECEIVED the other's message.
#
# Both RNS ends are the pinned reference RNS from the venv (validation/oracles/requirements.txt;
# $SMOKE_PYTHON if set, else the local reference venv) — genuine RNS-on-the-wire. Prints PASS or
# FAIL and exits accordingly.
set -u

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
source "$ROOT/validation/interop/lib/cargo-artifacts.sh"
DAEMON="$(cargo_debug_example "$ROOT/validation/integration/Cargo.toml" local_transit_daemon)"
VENV_PY="${SMOKE_PYTHON:-$ROOT/validation/.venv/rns-1.4.2/bin/python}"
PEER="$ROOT/validation/interop/peers/rns_transit_peer.py"
CLIENT="$ROOT/validation/interop/peers/rns_transit_client.py"
IFAC_HOSTILE="$ROOT/validation/interop/peers/rns_ifac_hostile.py"
PEER_LOG="$(mktemp)"
DAEMON_LOG="$(mktemp)"
CLIENT_LOG="$(mktemp)"
PEER_PID=""; DAEMON_PID=""; CLIENT_PID=""

cleanup() {
    for pid in "$CLIENT_PID" "$DAEMON_PID" "$PEER_PID"; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
}
trap cleanup EXIT

[ -x "$VENV_PY" ] || { echo "FAIL: reference venv python not found at $VENV_PY"; exit 1; }

# Two free loopback ports: the peer's TCP server, and the bridge's shared-instance port.
read -r PEER_TCP_PORT LOCAL_PORT <<EOF
$("$VENV_PY" - <<'PY'
import socket
def free():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p
print(free(), free())
PY
)
EOF
[ -n "${PEER_TCP_PORT:-}" ] && [ -n "${LOCAL_PORT:-}" ] || { echo "FAIL: could not allocate ports"; exit 1; }
echo "peer tcp=$PEER_TCP_PORT  bridge local=$LOCAL_PORT"

echo "building the bridge daemon example..."
cargo build --quiet --manifest-path "$ROOT/validation/integration/Cargo.toml" --example local_transit_daemon \
    || { echo "FAIL: daemon build"; exit 1; }

# 1) The remote RNS peer (TCP server). Wait until its destination is up.
PEER_TCP_PORT="$PEER_TCP_PORT" "$VENV_PY" "$PEER" > "$PEER_LOG" 2>/dev/null &
PEER_PID=$!
for _ in $(seq 1 100); do grep -q "PEER_DEST" "$PEER_LOG" && break; sleep 0.2; done
DEST="$(grep -o 'PEER_DEST [0-9a-f]*' "$PEER_LOG" | head -1 | cut -d' ' -f2)"
[ -n "$DEST" ] || { echo "FAIL: the RNS peer never came up"; tail -20 "$PEER_LOG"; exit 1; }
echo "peer up, dest=$DEST"

if [ -n "${PRNS_IFAC_NETWORK_NAME:-}" ]; then
    for mode in missing wrong; do
        HOSTILE_LOG="$(mktemp)"
        PEER_TCP_PORT="$PEER_TCP_PORT" "$VENV_PY" "$IFAC_HOSTILE" "$mode" > "$HOSTILE_LOG" 2>&1
        grep -q "HOSTILE_SENT $mode" "$HOSTILE_LOG" || { echo "FAIL: $mode IFAC peer did not exercise the TCP interface"; cat "$HOSTILE_LOG"; exit 1; }
        ! grep -q "HOSTILE_PEER_ANNOUNCE\|HOSTILE_LINK_ACTIVE" "$HOSTILE_LOG" || { echo "FAIL: $mode IFAC peer received authenticated traffic"; cat "$HOSTILE_LOG"; exit 1; }
        ! grep -q "HOSTILE_RECEIVED" "$PEER_LOG" || { echo "FAIL: $mode IFAC peer injected an announce"; cat "$PEER_LOG"; exit 1; }
    done
    echo "IFAC_REJECTION_OK missing=1 incorrect=1"
fi

# 2) The Prns bridge: holds the local bus, dials the peer over TCP.
PRNS_LOCAL_PORT="$LOCAL_PORT" PRNS_PEER_ADDR="127.0.0.1:$PEER_TCP_PORT" \
    "$DAEMON" > "$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 50); do grep -q "READY" "$DAEMON_LOG" && break; sleep 0.2; done
grep -q "READY" "$DAEMON_LOG" || { echo "FAIL: bridge never became READY"; cat "$DAEMON_LOG"; exit 1; }
echo "bridge up; starting the local client..."

# 3) The local RNS client: discovers the peer across the bridge and sends it a packet.
PRNS_LOCAL_PORT="$LOCAL_PORT" "$VENV_PY" "$CLIENT" > "$CLIENT_LOG" 2>/dev/null &
CLIENT_PID=$!

# 4) Wait for both ends to receive the other's multi-part resource across the bridge.
for _ in $(seq 1 320); do
    grep -q "RESOURCE_OK" "$PEER_LOG" && grep -q "RESOURCE_OK" "$CLIENT_LOG" && break
    sleep 0.25
done

OUT_OK=""; IN_OK=""
grep -q "RESOURCE_OK" "$PEER_LOG" && OUT_OK=1
grep -q "RESOURCE_OK" "$CLIENT_LOG" && IN_OK=1
EGRESS_METRICS="$(grep 'EGRESS_METRICS' "$DAEMON_LOG" | tail -1)"
[ -n "$EGRESS_METRICS" ] || { echo "FAIL: bridge did not publish egress diagnostics"; tail -30 "$DAEMON_LOG"; exit 1; }

if [ -n "$OUT_OK" ] && [ -n "$IN_OK" ]; then
    echo "PASS: real RNS apps transferred multi-part resources through the shared instance both ways"
    echo "  local client -> peer: $(grep -o 'RESOURCE_OK [0-9]*' "$PEER_LOG" | head -1)"
    echo "  peer -> local client (inbound transit): $(grep -o 'RESOURCE_OK [0-9]*' "$CLIENT_LOG" | head -1)"
    echo "  bridge: $EGRESS_METRICS"
    exit 0
fi

echo "FAIL: resource transfer across the bridge was not bidirectional"
echo "  local client -> peer: $([ -n "$OUT_OK" ] && echo delivered || echo MISSING)"
echo "  peer -> local client: $([ -n "$IN_OK" ] && echo delivered || echo MISSING)"
echo "--- bridge log (tail) ---"; tail -30 "$DAEMON_LOG"
echo "--- client log (tail) ---"; tail -20 "$CLIENT_LOG"
echo "--- peer log (tail) ---"; tail -20 "$PEER_LOG"
exit 1

#!/usr/bin/env bash
set -euo pipefail

echo "wifi-direct hwsim smoke: two netns-isolated nodes, each with a private bus and its own wpa_supplicant"
echo "the system wpa_supplicant routes all D-Bus P2P to one management interface, so isolation is required for a two-node bench"
echo "requires sudo; loads mac80211_hwsim; the real radio and its connection stay untouched"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$REPO_ROOT/prns-interfaces/impls/tokio/target/debug/examples/wifi_direct_linux"
NS_A=prns-wd-a
NS_B=prns-wd-b
BUS_A=/tmp/$NS_A.bus
BUS_B=/tmp/$NS_B.bus
WPA_BIN="$(command -v wpa_supplicant || echo /sbin/wpa_supplicant)"
PIDS=()

cleanup() {
    for pid in "${PIDS[@]:-}"; do
        sudo kill "$pid" 2>/dev/null || true
    done
    sleep 0.5
    sudo ip netns del "$NS_A" 2>/dev/null || true
    sudo ip netns del "$NS_B" 2>/dev/null || true
    sudo rm -f "$BUS_A" "$BUS_B"
}
trap cleanup EXIT

sudo -v

cd "$REPO_ROOT/prns-interfaces/impls/tokio"
cargo build --example wifi_direct_linux --features wifi-direct

sudo modprobe -r mac80211_hwsim 2>/dev/null || true
sudo modprobe mac80211_hwsim radios=2

mapfile -t PHYS < <(for d in /sys/devices/virtual/mac80211_hwsim/hwsim*/ieee80211/*; do
    [ -e "$d" ] && basename "$d"
done | sort | head -2)

if [ "${#PHYS[@]}" -lt 2 ]; then
    echo "expected two hwsim phys, found: ${PHYS[*]:-none}" >&2
    exit 1
fi
echo "hwsim phys: ${PHYS[0]} ${PHYS[1]}"

sudo ip netns del "$NS_A" 2>/dev/null || true
sudo ip netns del "$NS_B" 2>/dev/null || true
sudo ip netns add "$NS_A"
sudo ip netns add "$NS_B"
sudo iw phy "${PHYS[0]}" set netns name "$NS_A"
sudo iw phy "${PHYS[1]}" set netns name "$NS_B"
sudo ip netns exec "$NS_A" ip link set lo up
sudo ip netns exec "$NS_B" ip link set lo up

IF_A="$(sudo ip netns exec "$NS_A" sh -c 'ls /sys/class/net | grep -v "^lo$" | head -1')"
IF_B="$(sudo ip netns exec "$NS_B" sh -c 'ls /sys/class/net | grep -v "^lo$" | head -1')"
echo "node A: $IF_A in $NS_A; node B: $IF_B in $NS_B"

for NS in "$NS_A" "$NS_B"; do
    sudo ip netns exec "$NS" sysctl -qw \
        net.ipv6.conf.all.disable_ipv6=0 \
        net.ipv6.conf.default.disable_ipv6=0 \
        net.ipv6.conf.all.addr_gen_mode=0 \
        net.ipv6.conf.default.addr_gen_mode=0 || true
done

OBS=/tmp/prns-wd-observer.log
: > "$OBS"
(
    for _ in $(seq 80); do
        {
            echo "=== $(date +%T) $NS_A ==="
            sudo ip -n "$NS_A" addr show
            echo "=== $(date +%T) $NS_B ==="
            sudo ip -n "$NS_B" addr show
        } >> "$OBS" 2>&1
        sleep 3
    done
) &
PIDS+=($!)
echo "namespace address observer writing to $OBS"

sudo rm -f "$BUS_A" "$BUS_B"
sudo dbus-daemon --session --address="unix:path=$BUS_A" --nofork --nopidfile &
PIDS+=($!)
sudo dbus-daemon --session --address="unix:path=$BUS_B" --nofork --nopidfile &
PIDS+=($!)
sleep 0.5

sudo ip netns exec "$NS_A" env DBUS_SYSTEM_BUS_ADDRESS="unix:path=$BUS_A" "$WPA_BIN" -u &
PIDS+=($!)
sudo ip netns exec "$NS_B" env DBUS_SYSTEM_BUS_ADDRESS="unix:path=$BUS_B" "$WPA_BIN" -u &
PIDS+=($!)
sleep 1

sudo ip netns exec "$NS_A" env DBUS_SYSTEM_BUS_ADDRESS="unix:path=$BUS_A" RUST_LOG="${RUST_LOG:-info}" \
    "$EXAMPLE" "$IF_A" announce &
PIDS+=($!)

sudo ip netns exec "$NS_B" env DBUS_SYSTEM_BUS_ADDRESS="unix:path=$BUS_B" RUST_LOG="${RUST_LOG:-info}" \
    timeout 150 "$EXAMPLE" "$IF_B" expect

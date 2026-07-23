#!/usr/bin/env bash
set -euo pipefail

# Live proof of Wi-Fi Direct Single-Channel Concurrency (SCC) on virtual radios.
#
# Three netns-isolated mac80211_hwsim radios sharing one virtual medium:
#   - ns_ap: a hostapd access point on channel 6 (the "STA channel")
#   - ns_a / ns_b: two Prns nodes, each running its OWN wpa_supplicant that BOTH
#     associates to the AP as a station AND does Wi-Fi Direct P2P over the ctrl
#     socket (the SupplicantBackend attach-mode path). Because the STA sits on
#     ch6, decide() forms the P2P group owner co-channel on ch6 too.
#
# Proves: the hosting node's GO netdev comes up on channel 6 (SCC), and an RNS
# announce crosses the formed group. hwsim only; the real radio/internet are
# never touched.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$REPO_ROOT/validation/interop/lib/cargo-artifacts.sh"
EXAMPLE="$(cargo_debug_example "$REPO_ROOT/prns-interfaces/impls/tokio/Cargo.toml" wifi_direct_linux)"
WPA_BIN="$(command -v wpa_supplicant || echo /usr/sbin/wpa_supplicant)"
HOSTAPD_BIN="$(command -v hostapd || echo /usr/sbin/hostapd)"

NS_AP=prns-scc-ap
NS_A=prns-scc-a
NS_B=prns-scc-b
SSID=PrnsBench
PSK=prnsbench123
CHANNEL=6
FREQ=2437

OBS=/tmp/prns-wd-scc-observer.log
LOG_A=/tmp/prns-wd-scc-a.log
LOG_B=/tmp/prns-wd-scc-b.log
HOSTAPD_CONF=/tmp/prns-scc-hostapd.conf
HOSTAPD_LOG=/tmp/prns-scc-hostapd.log

PIDS=()

cleanup() {
    for pid in "${PIDS[@]:-}"; do sudo kill "$pid" 2>/dev/null || true; done
    sleep 0.5
    for ns in "$NS_AP" "$NS_A" "$NS_B"; do sudo ip netns del "$ns" 2>/dev/null || true; done
    sudo rm -rf /run/prns_wpa_* 2>/dev/null || true
    sudo modprobe -r mac80211_hwsim 2>/dev/null || true
}
trap cleanup EXIT

echo "wifi-direct SCC hwsim bench: hostapd AP on ch$CHANNEL + two ctrl-socket nodes (STA+P2P)"
sudo -v

if [ ! -x "$EXAMPLE" ]; then
    echo "example binary not found at $EXAMPLE" >&2
    echo "build it first as your user (not root):" >&2
    echo "  (cd $REPO_ROOT/prns-interfaces/impls/tokio && cargo build --locked --example wifi_direct_linux --features \"wifi-direct log\")" >&2
    exit 1
fi

sudo modprobe -r mac80211_hwsim 2>/dev/null || true
sudo modprobe mac80211_hwsim radios=3

mapfile -t PHYS < <(for d in /sys/devices/virtual/mac80211_hwsim/hwsim*/ieee80211/*; do
    [ -e "$d" ] && basename "$d"
done | sort | head -3)
if [ "${#PHYS[@]}" -lt 3 ]; then
    echo "expected three hwsim phys, found: ${PHYS[*]:-none}" >&2
    exit 1
fi
echo "hwsim phys: ${PHYS[*]}"

for ns in "$NS_AP" "$NS_A" "$NS_B"; do
    sudo ip netns del "$ns" 2>/dev/null || true
    sudo ip netns add "$ns"
done
sudo iw phy "${PHYS[0]}" set netns name "$NS_AP"
sudo iw phy "${PHYS[1]}" set netns name "$NS_A"
sudo iw phy "${PHYS[2]}" set netns name "$NS_B"
for ns in "$NS_AP" "$NS_A" "$NS_B"; do sudo ip netns exec "$ns" ip link set lo up; done

IF_AP="$(sudo ip netns exec "$NS_AP" sh -c 'ls /sys/class/net | grep -v "^lo$" | head -1')"
IF_A="$(sudo ip netns exec "$NS_A" sh -c 'ls /sys/class/net | grep -v "^lo$" | head -1')"
IF_B="$(sudo ip netns exec "$NS_B" sh -c 'ls /sys/class/net | grep -v "^lo$" | head -1')"
echo "AP=$IF_AP ($NS_AP)  A=$IF_A ($NS_A)  B=$IF_B ($NS_B)"

# Clients resolve the group owner over an IPv6 link-local (EUI-64), like the D-Bus smoke.
for ns in "$NS_A" "$NS_B"; do
    sudo ip netns exec "$ns" sysctl -qw \
        net.ipv6.conf.all.disable_ipv6=0 \
        net.ipv6.conf.default.disable_ipv6=0 \
        net.ipv6.conf.all.addr_gen_mode=0 \
        net.ipv6.conf.default.addr_gen_mode=0 || true
done

cat > "$HOSTAPD_CONF" <<EOF
interface=$IF_AP
driver=nl80211
ssid=$SSID
hw_mode=g
channel=$CHANNEL
wpa=2
wpa_passphrase=$PSK
wpa_key_mgmt=WPA-PSK
rsn_pairwise=CCMP
EOF
sudo ip netns exec "$NS_AP" "$HOSTAPD_BIN" "$HOSTAPD_CONF" > "$HOSTAPD_LOG" 2>&1 &
PIDS+=($!)
echo "hostapd started on $IF_AP ch$CHANNEL ssid=$SSID (log: $HOSTAPD_LOG)"
sleep 2

write_conf() {
    local if="$1"
    cat > "/tmp/prns-scc-wpa-$if.conf" <<EOF
ctrl_interface=DIR=/run/prns_wpa_$if
update_config=0
device_name=Prns-$if
device_type=1-0050F204-1
p2p_go_intent=15
p2p_listen_reg_class=81
p2p_listen_channel=$CHANNEL
p2p_oper_reg_class=81
p2p_oper_channel=$CHANNEL
network={
    ssid="$SSID"
    psk="$PSK"
    key_mgmt=WPA-PSK
}
EOF
}

for pair in "$NS_A:$IF_A" "$NS_B:$IF_B"; do
    IFS=: read -r ns if <<< "$pair"
    sudo mkdir -p "/run/prns_wpa_$if"
    write_conf "$if"
    sudo ip netns exec "$ns" "$WPA_BIN" -Dnl80211 -i "$if" -c "/tmp/prns-scc-wpa-$if.conf" \
        > "/tmp/prns-scc-wpa-$if.log" 2>&1 &
    PIDS+=($!)
done
echo "node supplicants started (logs: /tmp/prns-scc-wpa-*.log)"

wait_assoc() {
    local ns="$1" if="$2"
    for _ in $(seq 30); do
        if sudo ip netns exec "$ns" wpa_cli -p "/run/prns_wpa_$if" -i "$if" status 2>/dev/null \
            | grep -q "wpa_state=COMPLETED"; then
            local f
            f="$(sudo ip netns exec "$ns" wpa_cli -p "/run/prns_wpa_$if" -i "$if" status 2>/dev/null \
                | grep -m1 '^freq=' || true)"
            echo "  $if associated ($f)"
            return 0
        fi
        sleep 1
    done
    echo "  $if did NOT associate to the AP in time" >&2
    return 1
}
echo "waiting for both nodes to associate to the AP on ch$CHANNEL ..."
wait_assoc "$NS_A" "$IF_A"
wait_assoc "$NS_B" "$IF_B"

: > "$OBS"
(
    for _ in $(seq 60); do
        {
            echo "=== $(date +%T) ==="
            for pair in "$NS_A:$IF_A" "$NS_B:$IF_B"; do
                IFS=: read -r ns if <<< "$pair"
                echo "-- $ns $if --"
                sudo ip netns exec "$ns" wpa_cli -p "/run/prns_wpa_$if" -i "$if" status 2>/dev/null \
                    | grep -E "wpa_state|^freq=|^ssid=" || true
                for pif in $(sudo ip netns exec "$ns" sh -c 'ls /sys/class/net 2>/dev/null | grep "^p2p-"' 2>/dev/null); do
                    echo "   group iface $pif:"
                    sudo ip netns exec "$ns" iw dev "$pif" info 2>/dev/null \
                        | grep -E "type|channel|ssid" || true
                done
            done
        } >> "$OBS" 2>&1
        sleep 3
    done
) &
PIDS+=($!)
echo "observer writing to $OBS"

group_on_channel() {
    local found=0
    for pair in "$@"; do
        IFS=: read -r ns if <<< "$pair"
        for pif in $(sudo ip netns exec "$ns" sh -c 'ls /sys/class/net 2>/dev/null | grep "^p2p-"' 2>/dev/null); do
            info="$(sudo ip netns exec "$ns" iw dev "$pif" info 2>/dev/null || true)"
            echo "  $ns $pif:" >&2
            echo "$info" | grep -E "type|channel|ssid" | sed 's/^/    /' >&2 || true
            echo "$info" | grep -q "$FREQ MHz" && found=1
        done
    done
    return $((1 - found))
}

if [ "${MODE:-host}" = form ]; then
    echo "MODE=form: node A (announce/PREFER_OWNER) + node B (expect/PREFER_CLIENT) forming a group ..."
    sudo ip netns exec "$NS_A" env HOPSPOT_WIFI_DIRECT_CTRL="/run/prns_wpa_$IF_A" \
        RUST_LOG="${RUST_LOG:-info}" "$EXAMPLE" "$IF_A" announce > "$LOG_A" 2>&1 &
    PIDS+=($!)
    echo "node B waiting up to 150s for the crossing (log: $LOG_B) ..."
    set +e
    sudo ip netns exec "$NS_B" env HOPSPOT_WIFI_DIRECT_CTRL="/run/prns_wpa_$IF_B" \
        RUST_LOG="${RUST_LOG:-info}" timeout 150 "$EXAMPLE" "$IF_B" expect 2>&1 | tee "$LOG_B"
    EXPECT_RC=${PIPESTATUS[0]}
    set -e

    echo
    echo "=== formation verdict ==="
    GROUP_ON_CHANNEL=0
    group_on_channel "$NS_A:$IF_A" "$NS_B:$IF_B" && GROUP_ON_CHANNEL=1
    CROSSED=0
    grep -q "announce crossed the group" "$LOG_B" 2>/dev/null && CROSSED=1
    echo
    if [ "$GROUP_ON_CHANNEL" = 1 ] && [ "$CROSSED" = 1 ]; then
        echo "FORM_BENCH_PASS: two-node group co-channel on ch$CHANNEL ($FREQ MHz) AND announce crossed"
    elif [ "$GROUP_ON_CHANNEL" = 1 ]; then
        echo "FORM_BENCH_PARTIAL: group on ch$CHANNEL but no crossing (expect_rc=$EXPECT_RC)"
    else
        echo "FORM_BENCH_FAIL: no group on ch$CHANNEL (expect_rc=$EXPECT_RC); see $OBS $LOG_A $LOG_B"
    fi
else
    echo "MODE=host: node A forming its group owner directly on the STA channel (log: $LOG_A) ..."
    sudo ip netns exec "$NS_A" env HOPSPOT_WIFI_DIRECT_CTRL="/run/prns_wpa_$IF_A" \
        RUST_LOG="${RUST_LOG:-info}" "$EXAMPLE" "$IF_A" host > "$LOG_A" 2>&1 &
    PIDS+=($!)
    for _ in $(seq 25); do
        if sudo ip netns exec "$NS_A" sh -c 'ls /sys/class/net 2>/dev/null | grep -q "^p2p-"'; then break; fi
        sleep 1
    done
    sleep 2

    echo
    echo "=== SCC verdict (node A's group owner should sit on the STA channel) ==="
    GROUP_ON_CHANNEL=0
    group_on_channel "$NS_A:$IF_A" && GROUP_ON_CHANNEL=1
    echo "--- host self-test log ---"
    tail -8 "$LOG_A"
    echo
    if [ "$GROUP_ON_CHANNEL" = 1 ]; then
        echo "SCC_BENCH_PASS: group owner formed co-channel on ch$CHANNEL ($FREQ MHz) = the STA channel"
    else
        echo "SCC_BENCH_FAIL: no group owner on ch$CHANNEL; see $LOG_A and /tmp/prns-scc-wpa-$IF_A.log"
    fi
fi
echo "logs: $OBS  $LOG_A  $LOG_B  /tmp/prns-scc-wpa-*.log  $HOSTAPD_LOG"

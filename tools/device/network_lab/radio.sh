#!/bin/sh
# Temporary, channel-1 lab only. No UCI writes, default routes, or DNS edits.
set -eu
operation=${1:?operation required}
base=${2:?private /tmp/prns-wifi-lab-* directory required}
case "$base" in /tmp/prns-wifi-lab-*) ;; *) exit 2;; esac
case "$base" in *[!a-zA-Z0-9/_-]*) exit 2;; esac

stop() {
    if [ -e "$base/stopped" ]; then return; fi
    for role in host dhcp wpa dnsmasq hostapd timer; do
        pid=$(cat "$base/$role.pid" 2>/dev/null || true)
        case "$pid" in ''|*[!0-9]*) continue;; esac
        [ "$pid" != "$$" ] || continue
        [ -r "/proc/$pid/cmdline" ] || continue
        case "$(tr '\000' ' ' <"/proc/$pid/cmdline")" in
            *"$base"*) kill -TERM "$pid" 2>/dev/null || true;;
        esac
    done
    sleep 1
    if [ -f "$base/ap.role" ] || [ -f "$base/client.role" ]; then
        for chain in input forward; do
            for handle in $(nft -a list chain inet fw4 "$chain" 2>/dev/null | awk '/comment "prns-wifi-lab"/ {print $NF}'); do
                nft delete rule inet fw4 "$chain" handle "$handle" 2>/dev/null || true
            done
        done
    fi
    if [ -f "$base/ap.role" ]; then
        nft flush chain inet fw4 prns_lab_input 2>/dev/null || true
        nft delete chain inet fw4 prns_lab_input 2>/dev/null || true
        iw dev prnsap0 del 2>/dev/null || true
        ip link del br-prnslab 2>/dev/null || true
    fi
    if [ -f "$base/client.role" ]; then
        iw dev prnssta0 del 2>/dev/null || true
    fi
    if [ -f "$base/radio0.was-up" ]; then
        ubus call network.wireless up '{"device":"radio0"}'
    fi
    date >"$base/stopped"
}

case "$operation" in
    stop) stop; exit;;
    expire) sleep "${3:?timeout required}"; stop; exit;;
    ap|client) ;;
    *) exit 2;;
esac

[ ! -e "$base/started" ]
[ -f "$base/radio.sh" ]
[ -z "$(uci changes)" ]
[ ! -e /sys/class/net/prnsap0 ]
[ ! -e /sys/class/net/prnssta0 ]
[ ! -e /sys/class/net/br-prnslab ]
# A half-created stage is still cleaned up; the watchdog survives this SSH shell.
trap 'stop' EXIT
trap 'exit 1' HUP INT TERM
umask 077
ip -br addr >"$base/addresses.before"
ip route >"$base/routes.before"
iw dev >"$base/radios.before"
sha256sum /etc/config/network /etc/config/wireless /etc/config/firewall /etc/config/dhcp >"$base/config.before"
ubus call network.wireless status '{"device":"radio0"}' >"$base/radio0.before"
if [ "$(jsonfilter -i "$base/radio0.before" -e '@.radio0.up')" = true ]; then
    touch "$base/radio0.was-up"
fi
date >"$base/started"
sh "$base/radio.sh" expire "$base" 420 >"$base/timer.log" 2>&1 </dev/null &
echo $! >"$base/timer.pid"
ubus call network.wireless down '{"device":"radio0"}'
sleep 2

if [ "$operation" = ap ]; then
    ! nft list chain inet fw4 prns_lab_input >/dev/null 2>&1
    touch "$base/ap.role"
    ip link add br-prnslab type bridge
    ip link set br-prnslab address 02:50:52:4e:53:01
    ip addr add 172.23.73.1/24 dev br-prnslab
    ip link set br-prnslab up
    nft -f "$base/firewall.nft"
    iw phy phy0 interface add prnsap0 type __ap
    ip link set prnsap0 address "$(cat "$base/interface.mac")"
    hostapd -B -P "$base/hostapd.pid" "$base/hostapd.conf" >"$base/hostapd.log" 2>&1
    dnsmasq --conf-file="$base/dnsmasq.conf"
else
    touch "$base/client.role"
    nft -f "$base/firewall.nft"
    iw phy phy0 interface add prnssta0 type managed
    ip link set prnssta0 address "$(cat "$base/interface.mac")"
    ip link set prnssta0 up
    wpa_supplicant -D nl80211 -i prnssta0 -c "$base/wpa.conf" >"$base/wpa.log" 2>&1 </dev/null &
    echo $! >"$base/wpa.pid"
    sleep 1
    kill -0 "$(cat "$base/wpa.pid")"
fi
trap - EXIT HUP INT TERM
printf 'LAB_STARTED role=%s base=%s rollback_seconds=420\n' "$operation" "$base"

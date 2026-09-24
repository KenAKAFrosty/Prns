#!/bin/sh
# udhcpc hook: configure only the temporary client address, never a default route.
set -eu
[ "$interface" = prnssta0 ]
case "$1" in
    bound|renew)
        [ "$subnet" = 255.255.255.0 ]
        case "$ip" in 172.23.73.*) ;; *) exit 1;; esac
        ip addr replace "$ip/24" dev "$interface"
        ;;
    deconfig) ip -4 addr flush dev "$interface";;
esac

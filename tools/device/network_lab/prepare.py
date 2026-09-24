#!/usr/bin/env python3
"""Stage the tested, temporary G4/Heltec channel-1 network qualification."""
import argparse
import os
from pathlib import Path
import re
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--role', choices=['ap', 'client'], required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--mac', required=True)
    parser.add_argument('--psk-file', type=Path, required=True,
                        help='Private file containing the same 64-hex WPA2 PSK for all boards')
    args = parser.parse_args()
    if not re.fullmatch(r'prns-wifi-lab-[A-Za-z0-9_-]+', args.output.name):
        parser.error('output basename must be prns-wifi-lab- followed by letters, digits, _ or -')
    if not re.fullmatch(r'(?:[0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}', args.mac):
        parser.error('invalid MAC address')
    if int(args.mac[:2], 16) & 3 != 2:
        parser.error('MAC must be locally administered and unicast')
    if args.mac.lower() == '02:50:52:4e:53:01':
        parser.error('MAC is reserved for the AP bridge')
    key = args.psk_file.read_text().strip()
    if not re.fullmatch(r'[0-9a-fA-F]{64}', key):
        parser.error('PSK file must contain exactly 64 hexadecimal digits')
    base = '/tmp/' + args.output.name
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)

    def write(name, text):
        with open(args.output / name, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as file:
            file.write(text)

    for name in ['radio.sh', 'lease.sh']:
        shutil.copyfile(Path(__file__).with_name(name), args.output / name)
        (args.output / name).chmod(0o700)
    write('interface.mac', args.mac.lower() + '\n')
    if args.role == 'client':
        write('wpa.conf', f'''ctrl_interface={base}/ctrl
ap_scan=1
network={{
 ssid="Hopspot-Lab-0929"
 psk={key}
 scan_freq=2412
 key_mgmt=WPA-PSK
}}
''')
        write('firewall.nft', '''insert rule inet fw4 input iifname "prnssta0" meta l4proto { icmp, ipv6-icmp } counter accept comment "prns-wifi-lab"
insert rule inet fw4 input iifname "prnssta0" udp dport { 5353, 29716, 29717, 42671 } counter accept comment "prns-wifi-lab"
''')
        return
    write('hostapd.conf', f'''interface=prnsap0
bridge=br-prnslab
driver=nl80211
ctrl_interface={base}/ctrl
ssid=Hopspot-Lab-0929
hw_mode=g
channel=1
ieee80211n=1
wmm_enabled=1
beacon_int=100
max_num_sta=4
ap_isolate=0
auth_algs=1
wpa=2
wpa_key_mgmt=WPA-PSK
rsn_pairwise=CCMP
wpa_psk={key}
''')
    write('dnsmasq.conf', f'''port=0
interface=br-prnslab
bind-interfaces
except-interface=lo
dhcp-range=172.23.73.100,172.23.73.120,255.255.255.0,10m
dhcp-option=3
dhcp-option=6
dhcp-leasefile={base}/leases
pid-file={base}/dnsmasq.pid
log-facility={base}/dnsmasq.log
''')
    write('firewall.nft', '''add chain inet fw4 prns_lab_input
add rule inet fw4 prns_lab_input ct state established,related counter accept
add rule inet fw4 prns_lab_input meta l4proto { icmp, ipv6-icmp } counter accept
add rule inet fw4 prns_lab_input udp dport { 67, 5353, 29716, 29717, 42671 } counter accept
add rule inet fw4 prns_lab_input tcp dport { 42699, 4345, 4346 } counter accept
add rule inet fw4 prns_lab_input counter reject
insert rule inet fw4 input iifname "br-prnslab" jump prns_lab_input comment "prns-wifi-lab"
insert rule inet fw4 forward iifname "br-prnslab" counter drop comment "prns-wifi-lab"
insert rule inet fw4 forward oifname "br-prnslab" counter drop comment "prns-wifi-lab"
''')


if __name__ == '__main__':
    main()

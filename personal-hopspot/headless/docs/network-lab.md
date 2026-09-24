# Temporary 2.4 GHz network lab

These scripts reproduce the G4 AP / two Heltec station qualification. They are
lab tools, not a persistent installer. They replace the current `radio0` role
for at most seven minutes, then restore it through netifd. Keep a separate wired
management session open on every board. Existing 2.4 GHz clients disconnect
while the test runs. HaLoW and Ethernet are not reconfigured.

Prerequisites: the tested OpenWrt firmware, `radio0` mapping to `phy0`, `fw4`,
`nft`, `iw`, `ip`, `ubus`, `jsonfilter`, `hostapd`, `wpa_supplicant`, `dnsmasq`,
`udhcpc`, no pending UCI changes, and no existing `prns*` lab interfaces/rules.
Check the local channel-1 rules and ensure 172.23.73.0/24 does not overlap an
existing network. The fixed names and addresses permit only one lab at a time.

Generate one private random PSK file (64 hexadecimal digits) and use it for all
three stages. Do not commit it or generated configuration. For example:

```sh
umask 077
openssl rand -hex 32 > /tmp/hopspot-lab.psk
./tools/prns run device.network.lab.prepare -- --role ap --output /tmp/prns-wifi-lab-ap \
  --mac 02:50:52:4e:53:02 --psk-file /tmp/hopspot-lab.psk
./tools/prns run device.network.lab.prepare -- --role client --output /tmp/prns-wifi-lab-client-a \
  --mac 02:50:52:4e:53:11 --psk-file /tmp/hopspot-lab.psk
./tools/prns run device.network.lab.prepare -- --role client --output /tmp/prns-wifi-lab-client-b \
  --mac 02:50:52:4e:53:12 --psk-file /tmp/hopspot-lab.psk
```

Copy each directory to `/tmp` on its corresponding board, retaining its basename
and private permissions. Use fresh directory names for each run. Never reuse a
started/stopped stage. Unique locally administered MACs are required; the bridge
reserves `02:50:52:4e:53:01`. Start the AP first:

```sh
sh /tmp/prns-wifi-lab-ap/radio.sh ap /tmp/prns-wifi-lab-ap
```

Then start each client (substitute its stage name):

```sh
base=/tmp/prns-wifi-lab-client-a
sh "$base/radio.sh" client "$base"
udhcpc -i prnssta0 -f -n -q -t 5 -T 2 -s "$base/lease.sh" -p "$base/dhcp.pid"
ip -4 addr show dev prnssta0
```

Association can take several seconds. Require a successful lease and association
before continuing. DHCP deliberately supplies neither a default gateway nor DNS;
the hook changes only the temporary interface address. The AP bridge contains
only `prnsap0`. Its input firewall allows the qualification transport ports and
ICMP/DHCP; forwarding between that bridge and other interfaces is dropped.

Stage the reviewed Hopspot binary separately, verify its hash, and run it with
`--state-dir BASE/state --listen 172.23.73.1:4345 --auto-wifi-device br-prnslab
--run-for 120`. Redirect logs under BASE and save its PID in `BASE/host.pid` so
cleanup owns it. Run `fetch_page` on both clients against port 42699 and the
node-page destination from the host's ready line. Verify the entire page, client
to client reachability, blocked SSH, blocked forwarding, and discovery captures.
No WebSocket listener is started merely by allowing port 4346.

The watcher cleans up after 420 seconds. To stop sooner on each board:

```sh
sh "$base/radio.sh" stop "$base"
```

To test the same automatic cleanup path sooner, run `radio.sh expire BASE 5`.
After cleanup, allow netifd several seconds to restore the original 2.4 GHz
interfaces. Verify `sha256sum -c BASE/config.before`, addresses, routes, empty
`uci changes`, absence of lab interfaces/firewall rules/processes, and the original
Hopspot page. Preserve identities and evidence; remove only stopped test binaries
and private lab credentials when finished. Snapshots under BASE can contain
existing Wi-Fi credentials and must stay private.

The process watchdog is not crash-proof recovery. Persistent installation still
needs its own boot/recovery qualification. See the
[hardware results](qualification/ap-two-client-g4-2026-09-29.md).

## Station-mode recovery

Reverse the roles: generate an AP stage for a Heltec and a client stage for the
G4. The station's default lab firewall admits discovery but does not open TCP
listeners. For this qualification, append the following rule to the G4 stage's
`firewall.nft` before starting it:

```nft
insert rule inet fw4 input iifname "prnssta0" tcp dport { 42699, 4345 } counter accept comment "prns-wifi-lab"
```

After association and DHCP, bind the temporary Hopspot to the obtained station
address on port 4345, select `--auto-wifi-device prnssta0`, and save its PID as
above. Probe port 42699 from the AP. The AP input chain must allow
`ct state established,related` before its final rejection: response packets to
AP-originated connections target ephemeral ports. The generator includes this
rule. New connections to unlisted administrative ports remain rejected.

Save `/proc/PID/stat` before the outage. Stop only the staged hostapd process,
checking its PID and command line against the stage directory first. Keep the
AP's bridge, DHCP server and wired management intact. Confirm station carrier is
zero and the AP cannot ping it. Restart hostapd with the same private config and
PID-file options. Require reassociation, a complete page transfer to the same
identity, and unchanged Hopspot PID **and start time**. Capture recovered
Auto-WiFi beacons, then clean up both stages. Do not mistake this retained-address
outage for a DHCP renewal or changed-network test.

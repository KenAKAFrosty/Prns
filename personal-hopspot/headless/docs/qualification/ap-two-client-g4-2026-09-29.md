# Separate G4 AP with two Heltec clients

A temporary G4 2.4 GHz AP served complete Hopspot pages to two Heltec stations
on a dedicated LAN. Timed cleanup restored the original network roles on all
three boards. This qualifies short functional operation, not throughput,
long-term stability, automatic discovery-to-peering, or persistent installation.

## Configuration

Application commit: `85abcbb16`. Static MIPS executable: 5,334,876 bytes, SHA-256
`a11baef5eaec55c06bd4d6f0bcab50f5049cf94ed84d92f1172142036ac790ee`.
Features included `wifi-halow`, `websocket`, and `wifi-auto`; this run attached
Auto-WiFi only to the separate ordinary Wi-Fi LAN.

The G4 used channel 1, HT20, WPA2-PSK/CCMP, WMM, beacon interval 100 TU and
client isolation disabled. Each board had one active 2.4 GHz role. Unique local
MACs were assigned to the temporary interfaces. `br-prnslab` contained only the
new AP port and owned 172.23.73.1/24. Ethernet management and existing HaLoW
interfaces stayed in their original networks. DHCP provided no default route or
DNS server. Neither Mac Wi-Fi nor persistent UCI configuration was changed.

The two clients obtained 172.23.73.115 and .116. Hopspot listened explicitly on
172.23.73.1:4345, with automatic rendezvous on port 42699 and
`--auto-wifi-device br-prnslab`. Client probes used that explicit IPv4 rendezvous
endpoint; they did not implement automatic discovery themselves.

## Observations

| Check | Result |
| --- | --- |
| Heltec A exact page | 3,542 bytes verified; reported RTT 133 ms |
| Heltec B exact page | 3,542 bytes verified; reported RTT 79 ms |
| A → AP ping | 3/3 replies; average 2.523 ms |
| B → AP ping | 3/3 replies; average 4.819 ms |
| A → B ping | 3/3 replies; average 5.694 ms |
| Client → AP SSH | Rejected; input reject counter incremented once |
| B → A's HaLoW address, routed through AP | 0/3 replies; AP forward-drop counter increased by exactly 3 packets / 252 bytes |
| AP LAN discovery capture | 16 packets; Auto-WiFi IPv6 beacons and IPv4/IPv6 mDNS queries visible |
| Simultaneous HaLoW capture | Zero matching UDP 29716 packets from the AP bridge addresses, zero kernel drops |
| G4 memory sample | MemAvailable 17,728 KiB; no swap |
| Original G4 Hopspot after rollback | Exact 3,542-byte page verified; reported RTT 51 ms |

Page-probe RTT is an application measurement, not a throughput estimate. The
22-second discovery window proves observed delivery and limited exclusion of
matching Auto-WiFi beacons; it is not an exhaustive packet-leakage audit. The
capture showed mDNS queries, not independent service resolution. Independent
browse/resolve was qualified separately in the
[DNS-SD test](auto-wifi-dnssd-g4-2026-09-29.md).

## Failed approaches and working changes

New interfaces initially failed to come up using inherited MAC addresses.
Assigning unique locally administered MACs allowed them to start on all three
boards. The vendor `wpa_supplicant` does not support `-f` logging and printed
usage while exiting successfully. The lab now redirects ordinary process output
and checks that the supplicant remains alive.

With the original Heltec APs still active, both added station interfaces
completed WPA2 association but repeatedly disconnected with beacon loss before
DHCP completed. The AP saw DISCOVER and sent OFFER, but no lease was acquired.
Stopping only the existing `radio0` roles through scoped netifd calls allowed
both stations to acquire leases and pass the tests above. This is evidence for
using one role per radio as the initial baseline, not proof of the underlying
coexistence failure's cause. Advertised interface-combination capability alone
is insufficient. AP+STA operation remains unqualified.

## Rollback and reproduction

Each board started a 420-second watcher before changing its 2.4 GHz role. After
testing, the same expiry path was invoked with a five-second delay. All three
wrote their stopped marker, removed the temporary interfaces/rules/processes,
and restored their prior roles. Hashes of network, wireless, firewall and DHCP
configuration matched on every board; pending UCI changes remained empty.
Addresses and routes returned to the baseline, including Heltec A's existing
HaLoW default route. G4's original TCP Hopspot continued serving its page.
Board clocks were unsynchronized; report dates use the host session date.

The [lab scripts](../network-lab.md) retain the tested runtime
configuration and cleanup procedure. Generated private staging files, credentials
and raw device snapshots are not repository artifacts. This is a RAM-only lab,
not a network migration or installation service.

Next: qualify a Hopspot running in station mode, reconnect/upstream loss,
automatic peer establishment, then sustained resource use. Keep AP+STA
coexistence separate until its failure is understood. Native HaLoW transport was
not attached to this test process; concurrent native HaLoW routing with the new
AP layout still needs an end-to-end run.

## Repository checks

Both shell scripts pass `sh -n`; `validation/run.py verify` and `git diff --check`
pass. The staging generator was exercised for all three roles/addresses, and
its output matched the files actually used on each board after normalizing only
the freshly generated private PSK. Stopped lab executable links were removed
from all three boards afterward; identity/state and evidence were retained.
No Rust implementation changed in this qualification.

The later [station recovery smoke](station-recovery-g4-2026-09-30.md) corrected
the lab AP firewall for AP-originated connections: established replies must be
accepted before its final rejection. The original client-to-AP results above
remain valid; this reverse direction was not exercised in that run.

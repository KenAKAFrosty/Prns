# DHCP address change and automatic-route investigation

The G4 retained its running Hopspot and served its complete page after obtaining
a different DHCP address. A separate cold-start routed-page test timed out in
both directions. Discovery tokens crossed the radio, but that does not yet
qualify automatic peer membership or routed application recovery.

## Setup and successful address change

The application remained the reviewed static MIPS executable, SHA-256
`a11baef5eaec55c06bd4d6f0bcab50f5049cf94ed84d92f1172142036ac790ee`.
Both boards ran fresh private Hopspot identities and selected only their lab
Wi-Fi devices for Auto-WiFi. Heltec A provided the isolated AP; the G4 was its
station. Ethernet management and existing HaLoW configuration were retained.
The lab AP used the corrected established-reply firewall rule from the preceding
[station recovery test](station-recovery-g4-2026-09-30.md).

The G4 initially obtained 172.23.73.110/24. Its explicit listener bound the stable
wired address 192.168.12.1:4345; Auto-WiFi owned the wildcard rendezvous listener
on port 42699 and selected only `prnssta0`. The Heltec's host listened on its
wired 10.42.0.1:4345 and selected only `br-prnslab`.

To force a different lease, the private lab dnsmasq was stopped, its private
lease file cleared, and a reservation for station MAC `02:50:52:4e:53:21` at
172.23.73.119 was added. After restarting that private DHCP server, the lab
udhcpc was explicitly invoked again with its address-only lease hook. It obtained
172.23.73.119/24; the old .110 address was absent and management routes remained.
This is forced DHCP reacquisition on the same subnet, not unattended lease
renewal, lease expiry, or movement to a different network.

The Heltec probe then connected to 172.23.73.119:42699 and verified all 3,542
page bytes, reporting RTT 54 ms and exit status zero. G4 PID 9010 and process
start tick 6877301 were unchanged. The destination remained
`8c5f2150a60c163022956f08a1235d40`. No Hopspot restart was required.

## Cold-start routed probes

The laptop attached only to one board's wired TCP listener and requested the
other board's page. No remote Wi-Fi endpoint was supplied to either Hopspot.
This tests application routing through automatically discovered connectivity,
which is more demanding than directly querying the remote rendezvous listener.

| Entry | Requested destination | Initial lease | Changed G4 lease |
| --- | --- | --- | --- |
| G4 wired TCP | Heltec page | Path discovery timeout | Path discovery timeout |
| Heltec wired TCP | G4 page | Path discovery timeout | Path discovery timeout |

A discovery capture saw IPv6 multicast beacons in both directions and unicast
reverse tokens on UDP 29717 in both directions. Firewall counters admitted the
traffic. A 22-second concurrent capture during repeated routed probes selected
UDP 42671 and TCP 42699 and captured zero packets, with zero kernel drops.
Thus the observed failure precedes Wi-Fi data transfer; these measurements do
not support blaming radio throughput or DHCP for it.

Code inspection identifies a cold-start announcement gap that must be addressed
before treating these probes as a peer-recovery test:

- `headless/src/main.rs` runs periodic destination announcements only when a
  native HaLoW attachment is present, targeting that attachment. These Auto-WiFi
  hosts had no such attachment and no seeded remote destination routes.
- The TCP interface uses `InterfaceMode::PointToPoint`; the default protocol
  policy disables recursive unknown-path forwarding. The policy in
  `prns-core/src/routing/ingress/path_requests.rs` therefore does not make an
  incoming unknown-path request flood every ordinary network interface.

This explains why the wired routed probe can fail even if discovery exchanges
are healthy. It does **not** independently establish that Auto-WiFi's peer
objects were admitted and remained healthy; direct Auto-WiFi-originated path
requests or membership instrumentation are still needed to isolate that claim.
No routing or announcement policy was changed during this experiment.

## Next qualification

Add explicit bounded application announcements on the intended Auto-WiFi
connections, preserving the HaLoW broadcast rule and avoiding unrelated
interfaces. Prove an initially empty node learns the remote destination without
an explicit endpoint or a seeded route. Then repeat the routed page checks
before and after departure/rejoin and address changes, instrumenting membership
and data traffic. Do not change generic recursive-path defaults merely to make
this particular probe pass.

For address-change reproduction, use the existing private network-lab staging
procedure, stable wired explicit listeners, and scoped Auto-WiFi selection. Change
only the private lab DHCP reservation/lease file, run the supplied udhcpc hook,
verify removal of the old address, compare PID plus start time, and request the
same destination at the new rendezvous address. Keep the rollback deadline and
configuration-hash checks from the [lab recipe](../network-lab.md).

## Restoration

Both stages were stopped through the shared rollback function. All four saved
UCI configuration hashes matched on both boards, pending UCI changes stayed
empty, temporary interfaces/rules/processes disappeared, and original radio
roles and routes returned. Board clocks were unsynchronized; the report date
uses the host session. Private credentials, identity state and raw captures
remain outside the repository.

The original G4 Hopspot subsequently verified its 3,542-byte page (76 ms probe
RTT). Stopped test binaries were removed to recover tmpfs memory. Repository
validation registry and diff-whitespace checks passed; this follow-up changed
qualification documentation only.

The [announcement follow-up](auto-wifi-routed-g4-2026-09-30.md) implements the
scoped announcement fix and passes all six routed-page checks with fresh states.

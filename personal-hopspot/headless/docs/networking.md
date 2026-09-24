# Hopspot LAN networking

OpenWrt owns AP/client association, addresses, DHCP, routing, and firewall policy.
Hopspot attaches transports to those configured networks. Browser assets are
optional and can remain off the board; neither TCP nor Auto-WiFi needs them.

## Choose the network role

| Role | OpenWrt responsibility | Hopspot responsibility |
| --- | --- | --- |
| Standalone access point | Dedicated LAN, AP authentication, DHCP, firewall | Auto-WiFi on that LAN; optional explicit TCP/WebSocket listeners |
| Home-network client | Join the chosen Wi-Fi, obtain an address; do not run DHCP on the upstream LAN | Auto-WiFi on the client LAN; bind other listeners deliberately |
| AP and client together | Supported single-channel combination, separate subnets, deliberate routing | Choose the participating networks explicitly |

Keep Ethernet management reachable while changing Wi-Fi. AP/client concurrency
shares channel and airtime on the 2.4 GHz radio; it is not two independent radios.
The HaLoW radio has its own profile and native transport.

## G4 audit, 2026-09-29

A read-only SSH audit found the address-owning `br-ahwlan` bridge at
192.168.12.1/24, containing `eth0.1`, `phy0-ap0`, and HaLoW `wlan0`.
The 2.4 GHz AP and station netdevs were down. The UCI `lan` interface referred to
an absent `br-lan`; its status reported `NO_DEVICE`. Do not copy a `br-lan` example
blindly, and do not equate a configured UCI section with a working interface.
There were no pending UCI changes.

The 2.4 GHz driver reported AP and managed modes and a combined limit of four
interfaces on one channel, with matching STA/AP beacon intervals. This is a
capability report, not a successful concurrent AP/client qualification.

Selecting `br-ahwlan` for Auto-WiFi currently includes HaLoW. That is acceptable
only as an explicit lab choice; it creates an additional IP transport over the
same radio. For the intended appliance, separate the ordinary LAN from native
HaLoW before enabling routine discovery. Do not remove the Ethernet management
path while restructuring this bridge.

## Discovery contract

Compile `wifi-auto` and supply `--auto-wifi-device NAME` once per selected
address-owning device. No selections means no Auto-WiFi supervisor or DNS-SD
provider. The same device allowlist feeds native DNS-SD and Auto-WiFi. Missing or
down devices can appear later; the configured log line does not assert readiness.

The existing runtime uses the public Reticulum discovery group, IPv6 link-local
multicast and reverse peering, unicast data, plus native DNS-SD and TCP rendezvous.
A discovery group is not a password or authentication mechanism. Reticulum
cryptographic identities remain responsible for authentication.

| Service | Port |
| --- | --- |
| IPv6 discovery / reverse peering | UDP 29716 / 29717 |
| Auto-WiFi data | UDP 42671 |
| TCP rendezvous | TCP 42699 |
| Native mDNS | UDP 5353 |

These are existing protocol defaults, not a new firewall-opening instruction.
The automatic rendezvous listener owns separate wildcard IPv4 and IPv6 sockets
on the same port. IPv4 peers use the selected LAN prefixes; IPv6 link-local
peers must carry a scope matching a selected interface. Loopback remains local. Device selection is not
an ingress firewall or subnet-isolation guarantee. Apply OpenWrt policy at the
network boundary. Existing peer/session bounds are inherited from Auto-WiFi;
appliance-specific memory and abuse/load qualification remain necessary.

Hopspot has no automatic application announcement policy. An authorized controller
explicitly requests `AnnounceSelf` through the [Remote Control API](remote-control.md).
Auto-WiFi discovery and DNS-SD still discover transport peers; they do not authorize
application announcements. HaLoW uses the same explicit control policy.

Native DNS-SD advertises the Auto-WiFi rendezvous service. It does not advertise
the independent headless WebSocket listener. Browsers do not get native mDNS
through this setting; a browser still needs its WebSocket endpoint.

The [initial attachment smoke](qualification/auto-wifi-g4-2026-09-29.md) passed
a direct rendezvous page transfer and captured multicast beacons. The [DNS-SD follow-up](qualification/auto-wifi-dnssd-g4-2026-09-29.md) now
qualifies service browse/resolve and exact page transfer to the advertised IPv6
endpoint.

The [separate AP qualification](qualification/ap-two-client-g4-2026-09-29.md)
passed two-client DHCP, exact page transfers, firewall boundary checks, scoped
discovery captures, and timed restoration. It used one role per 2.4 GHz radio;
AP+STA coexistence failed its initial smoke and remains unqualified. The
[station recovery smoke](qualification/station-recovery-g4-2026-09-30.md) passed
AP loss/rejoin and exact page recovery without restarting Hopspot. It also fixed
the lab AP firewall to admit replies to its own outbound connections. DHCP
renewal remains separate work. The [address-change investigation](qualification/address-discovery-g4-2026-09-30.md)
passed forced same-subnet DHCP reacquisition and a direct page transfer at the
new address. Cold-start routed probes timed out: discovery tokens were visible,
but application destination announcements were absent. Automatic routed recovery
was therefore still unqualified in that build. The
[announcement follow-up](qualification/auto-wifi-routed-g4-2026-09-30.md) demonstrated six routed transfers across cold start, AP rejoin,
and forced same-subnet DHCP address change using a temporary ticker. That ticker
is superseded by explicit controller commands; the report is historical lab evidence.

## Qualification sequence

1. Build the opt-in application and validate CLI selection, unchanged TCP/WS/HaLoW
   behavior, native lifecycle, and the MIPS static executable.
2. On the existing bridge, temporarily qualify rendezvous and discovery without
   changing radio configuration; label this as a shared-bridge lab test.
3. With an automatic timed rollback and retained Ethernet management, introduce
   a separate AP LAN and verify DHCP, firewall boundaries, multicast and actual
   two-client page transfers. Then exercise client mode on a chosen network.
4. Test AP/client coexistence, upstream loss, recovery, discovery withdrawal,
   unwanted-interface exclusion, session limits, and sustained memory/CPU use.

Do not ship a persistent network migration until the rollback and recovery paths
have passed on hardware. These application options do not modify UCI or install
services. The optional static browser bundle remains separate.

## RAM-only qualification hygiene

Before an upload, check both `df -k /tmp` and available RAM (`free` or
`/proc/meminfo`). On this board `/tmp` is tmpfs: free persistent overlay space
does not make room for RAM-backed candidate binaries. Budget the candidate,
upload staging, retained state, measured process memory, and operating headroom.
Repeated 5.3 MB copies caused an upload failure and sluggish management during
the DNS-SD work. After a process has stopped and evidence is saved locally,
remove its superseded executable and unused static assets. Preserve identities,
state, running applications, and unrelated device files.

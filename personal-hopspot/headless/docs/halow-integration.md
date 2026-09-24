# HaLoW Hopspot integration

The headless application has an opt-in Linux HaLoW CLI, explicit controller-driven
announcements, and a HaLoW page-fetch probe. The G4 development build includes the
feature; TCP-only use remains available. Portable identity/framing, bounded
first-frame peer admission, shared announce egress, and Linux/Tokio datagrams
exist under `interfaces::wifi_halow` and the `wifi-halow` feature.

Running-node tests cover the supervisor. Hardware checks passed on the vendor
AP/station connection and on a three-node open mesh, including concurrent page
transfers and application rejoin. A controlled datagram-medium test also covers
a forced two-hop route through the real Hopspot nodes and HaLoW supervisors.
Sustained load and a physically isolated two-radio-hop hardware test remain. See [the qualification report](qualification/halow-three-node-2026-09-29.md)
and [relay-broadcast follow-up](qualification/halow-relay-broadcast-2026-09-29.md)
for measurements and limits, and `thinknode-g4.md` for build/recovery procedures.
The dedicated [HaLoW simulation qualification](../../../validation/simulation/measurements/halow-qualification.md)
also covers replayable faults, actual peer queue/cap pressure, expiry,
adapter replacement and Remote Control while Resources overlap. It qualifies
the software transport; RF, firmware and durable installation still require
their separate device checks. The latest
[three-appliance check](qualification/halow-appliance-2026-10-01.md) passed
authenticated control and concurrent page work, and identified a radio lifecycle /
control-route recovery gap. The [recovery follow-up](qualification/halow-recovery-2026-10-02.md)
qualifies automatic packet rebinding, unavailable-route rediscovery and retained
identity/grants on all three boards without a radio-cycle process restart.
One page handshake timed out and recovered on explicit retry; radio delivery
remains fallible. The [deployment specification](halow-deployment.md)
defines the next installer transaction and storage strategy.

## Product shape

One headless Hopspot process owns its Reticulum identity, routing, persistence,
and attached interfaces. OpenWrt owns radio configuration, the network devices,
and service supervision. Install the application over the existing vendor OS.
Keep Morse firmware, drivers, board calibration, and regulatory data in place.

The web flasher is the user's starting point. The G4 entry initially explains
the application installation and provides the development build procedure.
A public download requires integration with signed release custody and hardware
qualification. Never label an arbitrary ELF as a firmware image or offer it to
LuCI's firmware upgrader.

## Native HaLoW contract

Use the Linux mesh network device's normal Ethernet data service, with one
shared packet socket. The lab used `AF_PACKET`/`SOCK_DGRAM`; no monitor injection
or custom chip firmware is needed for this path. Linux supplies the source MAC
alongside the received data. Keep platform FFI in `prns-ffi`, asynchronous I/O
in `prns-interfaces-tokio`, and transport policy independent of Linux.

`HaLowDevice` owns the stable configured supervisor; `HaLowRadioSource` supplies
owned packet-binding generations. The Linux source subscribes to link events
before binding and invalidates obsolete generations, including down/up events
in the same batch. Rebinding uses the existing bounded reconnect policy without
periodic station polling or changing the radio profile. A failed receive releases
the old socket and child lanes. A replacement reconstructs MAC-derived peers on
their first frame; neither local nor control announcements run automatically.

The peer key is a versioned, length-delimited stable local interface instance tag
plus the remote source MAC. Hash that with a new peer `InterfaceKind` through
the existing `InterfaceId::from_channel_tag` contract. It does not use the transient
Linux interface index. The scope is explicit, nonempty, and at most 64 bytes.
The user approved proceeding with this source-MAC design. HaLoW has distinct
interface kinds and a `RadioFamily::HaLow` status classification; unavailable
RSSI remains a radio indication, never `NotRadio`.

A valid first frame from an unseen source must attach the peer and deliver that
same frame in order. It must not wait for a station-table polling interval or a
new transport handshake. Bound peer admission and pending work. Reject malformed
frames, multicast source addresses, local outgoing echoes, and unrelated
EtherTypes before creating peers. MAC addresses identify transport neighbors;
Reticulum's cryptographic identities provide authentication. A changed MAC is
a new transport peer, and the same MAC on two configured local instances stays
distinct. Mesh peer events or occasional reconciliation inform liveness, not
whether an incoming frame has an identity.

Announces use one physical broadcast. Traffic with a particular destination
uses that peer's unicast MAC, retaining the driver's normal acknowledgements and
aggregation. A direct transmission must never silently fall back to broadcast
when its peer disappears. Announce fan-out must work even with zero known peers,
otherwise first discovery depends on already having discovered someone.

Tokio egress now recognizes a distinct shared broadcast channel grouped with
its peers by logical supervisor ID. Unrestricted announcements select that channel
once per radio before pacing and backpressure. Ordinary relayed announces also
select this channel, allowing the previous hop to overhear and deduplicate them.
Directed path responses and explicit excluded-recipient announcements retain
direct delivery; ordinary non-announce fleet fan-out selects peers only.
Existing point-to-point fleets keep their per-peer behavior. See the transport
README for frame layout, queue bounds, timeout, expiry, and current limitations.

The exact handling of `FanTarget::Only`, `AllExcept`, and non-announce fan-out
must be explicit. A physical broadcast cannot exclude one receiving station.
An excluded peer can physically hear it; routing suppression and duplicate
handling must prevent unwanted forwarding. Restricted recipient semantics that
actually require exclusion need directed delivery. Path requests and other
control fan-out need their own deliberate mapping; do not classify all fan-out
as an announce or all unicast data as an aggregate.

Keep 802.11s forwarding disabled for the initial Prns neighbor interface so the
overlay owns multi-hop forwarding. Enabling it later changes which endpoints a
MAC represents and needs separate qualification. The first implementation must
also specify a versioned wire discriminator and frame/MTU bounds: the lab's
experimental EtherType `0x88b6` is not a registered Prns assignment.

## Initial radio profile

The selected lab default is an open 802.11s mesh at 924 MHz center, 8 MHz width,
MCS2, long guard interval, power saving off, and mesh forwarding off. Keep the
profile explicit and editable after installation. Region and legal transmit
power must be validated against the actual board's regulatory configuration;
924 MHz is not a worldwide default. Do not dynamically negotiate a different
profile in this first implementation.

Measured receive payload goodput with vendor polling loops paused:

| Setting | Nominal PHY, long GI | Broadcast | Unicast |
| --- | ---: | ---: | ---: |
| MCS2 | 8.775 Mbps | 4.07–4.19 Mbps | 7.32–7.51 Mbps |
| MCS3 | 11.7 Mbps | 4.83–4.84 Mbps | 9.99–10.06 Mbps |
| MCS4 | 17.55 Mbps | 5.60–5.63 Mbps | 14.29–14.31 Mbps |

These are close-range, 1,400-byte probe results, not encrypted Reticulum resource
throughput or field-range results. A second receiver lost 2.955% in one MCS2
broadcast trial. See `scratch/halow-mcs234-2026-09-29/README.md` for the experiment
conditions and raw evidence in the development checkout. Shipping qualification
must retain sanitized evidence in durable release artifacts.

Vendor LED/button shell polling consumed appreciable host CPU. Production
integration should replace or properly disable the identified services while
preserving required button behavior; `SIGSTOP` is an experiment, not an installer
policy. Avoid hot-loop polling and periodic intrusive firmware-stat reads. Use
bounded queues and expose drops, airtime-relevant frame counts, resource usage,
and restart causes. Benchmark actual Prns transfers after raw-link qualification.

## Additional interfaces on the same appliance

| Capability | Existing foundation | Remaining appliance work |
| --- | --- | --- |
| Wired TCP | Headless host and lifecycle tests | Persistent service and install transaction |
| Wi-Fi AP or client | G4 reports both modes; vendor OpenWrt manages them | Explicit choice, retained Ethernet management, DHCP/firewall and concurrency checks |
| Auto-WiFi and mDNS | Opt-in device selection; G4 DNS-SD browse/resolve and IPv6 rendezvous qualified | Qualify multicast on AP/client; separate vendor bridge; budget sockets and discovery traffic |
| WebSocket and browser rendezvous | Opt-in bounded plain WebSocket listener; real browser-to-HaLoW transfers qualified | TLS/origin deployment, browser rendezvous/discovery, and long-running resource tests |
| Locally served browser node | Hashed static bundle; plain G4 HTTP origin, portable crypto, UI connect/close and browser identity reload qualified | Persistent storage does not fit current payload; HTTPS, other browsers, and appliance UI remain |
| ESP-NOW | Existing embedded support and 2.4 GHz Public Action TX/RX evidence | Linux vendor-category TX/RX and real Espressif interoperability remain unproven |

AP and client operation on one 2.4 GHz radio may share a channel and airtime.
ESP-NOW coexistence must respect that same channel. The HaLoW radio is separate.
The earlier 2.4 GHz test delivered all five short Public Action payloads without
association, but normal vendor-category 127 transmission was rejected. A later
combined raw-injection experiment rebooted the G4; the evidence does not identify
which radio or stage caused it. Public Action success alone does not establish
ESP-NOW compatibility. Further injection work needs crash visibility first.
No Bluetooth or LoRa capability should be inferred from these boards without
the corresponding hardware. Do not bundle every optional transport into the
minimal image until its code, RAM, and flash costs are measured.

The G4 currently has about 5.2 MiB free overlay. The static TCP-only application
is about 3.2 MiB. An update requiring two full copies and additional browser
assets does not fit that budget. Qualify an actual storage/update strategy before
advertising persistent installation with rollback. Keep private identity outside
replaceable application directories; bound logs and persisted routing state.

## Browser installation investigation

The connected G4 is reached through the Mac's built-in Ethernet. The two USB
network adapters enumerate as Realtek and ASIX networking devices. A WCH
`1a86:7523` serial adapter also enumerates, but its connection to a particular
board has not been established. No serial session was opened. The running G4
image has no `/sys/class/udc` device-controller interface.

[WebUSB requires a claimable USB interface](https://developer.chrome.com/docs/capabilities/build-for-webusb).
An Ethernet adapter does not provide access to the remote board's flash, and the
host network driver already owns its networking interface. A future USB gadget
or known UART may offer another route, but neither is currently qualified here.

Browser HTTP or WebSocket access could drive an authenticated local device API
or a loopback helper. Ordinary web pages do not have a raw SSH socket API.
Cross-origin access, authentication, mixed-content rules, and evolving
[local-network permissions](https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Local_network_access)
must be tested in each supported browser. Serving the browser app locally can
simplify origins, but plain LAN HTTP is not a secure context for every web API.
Public visitors' browser-node access and administrative installation must remain
separate privileges. Preserve signature verification and origin restrictions in
any helper; do not turn it into an unauthenticated local command proxy.

## Implementation gates

1. MAC-based identity, shared-radio egress, bounded first-frame admission, and the
   Linux packet adapter are implemented and covered by unit/runtime tests.
2. Three-node hardware captures confirm shared local/relayed announce broadcast
   and direct page traffic. Concurrent transfers and application restart passed.
   Radio-device recreation now passes the recovery follow-up with stable process
   and interface identity. Sustained load and long-running resource bounds remain.
3. The real Hopspot page transfers through wired TCP and native HaLoW. A
   [controlled two-hop regression](qualification/halow-controlled-two-hop.md)
   forces A→B→C, verifies immediate-hop MAC identity, and checks that removing B
   prevents progress. Hardware with independently verified endpoint isolation
   remains necessary before claiming a forced two-radio-hop radio qualification.
4. Plain WebSocket access has passed the [browser-to-HaLoW smoke](qualification/websocket-halow-2026-09-29.md), including eight concurrent clients.
   [Local browser hosting](qualification/browser-local-hosting-2026-09-29.md) also passed from G4 RAM.
   Add AP/client networking and discovery next; persistent asset storage remains unresolved.
   Keep ESP-NOW qualification separate from ordinary Wi-Fi operation.
5. Qualify persistent installation, identity retention across power loss, bounded
   state growth, full-storage behavior, and update rollback. Then publish a signed
   application download and promote the web entry out of development preview.

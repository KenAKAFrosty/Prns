# Auto-WiFi cold-start routing and recovery

> Historical lab experiment: its ten-second announcement ticker has been removed.
> Shipping Hopspots use explicit controller commands, not automatic announcements.


Six complete routed page transfers passed between the G4 and a Heltec with fresh
Hopspot states: both directions after cold start, AP loss/rejoin, and a changed
G4 DHCP address. This closes the cold-start announcement gap identified in the
[address/discovery investigation](address-discovery-g4-2026-09-30.md).

## Change and scope

Hopspot now originates its node-page and delivery announcements every ten seconds
on connected members owned by its explicitly configured Auto-WiFi supervisor.
The supervisor's member list includes UDP peers and discovered/accepted TCP
members. It avoids selecting unrelated TCP listeners or other radio families
by interface kind. No selected Auto-WiFi devices means no announcement task work.
Disconnected members are skipped, missed timer ticks are skipped, and normal
runtime announcement controls still apply. Newly attached peers are considered
on the next interval; this is not a guarantee of delivery within ten seconds.

The native HaLoW announcement loop and its broadcast target are unchanged.
Generic recursive path-request policy is unchanged. The fix supplies the missing
application destination announcements rather than enabling unknown-path flooding.

The tested application was built from `8b0975ad6` plus this announcement change,
using the checked-in G4 build task and qualified nightly/Zig toolchain. Static
MIPS binary: 5,339,772 bytes, SHA-256
`37a73770738c3caba42dcca6cccda6ac27e4d1e2529dbfd96fde340e5ad7ce79`.
Features: `wifi-halow`, `websocket`, `wifi-auto`. Size increased by 4,896 bytes
from the previous reviewed build. The ELF/ABI/static-link verifier passed.

## Hardware method

Heltec A provided the isolated channel-1 HT20 WPA2/CCMP AP. The G4 joined as a
station. Each board had one 2.4 GHz role; Ethernet and existing HaLoW networking
remained available. Timed rollback was armed before radio changes. The second
Heltec was untouched. No persistent UCI settings changed.

Each board ran the hash-verified candidate with a fresh private state directory
and a 330-second lifetime. G4 selected only `prnssta0` and exposed its explicit
TCP listener at 192.168.12.1:4345. Heltec selected only `br-prnslab` and exposed
its listener at 10.42.0.1:4345. Both restored zero routes, identities and tunnels.
Neither host received a configured remote Wi-Fi endpoint.

The laptop probe connected to one board's wired listener and requested the
other board's node-page destination. Every success established a Reticulum link
and verified the complete 3,542-byte response. This exercises automatically
established inter-board connectivity and routing, not a direct remote TCP probe.

| Phase | G4 entry → Heltec page | Heltec entry → G4 page |
| --- | ---: | ---: |
| Fresh identities, cold start | 46 ms | 145 ms |
| AP outage and automatic rejoin | 95 ms | 116 ms |
| G4 DHCP address changed | 83 ms | 53 ms |

These are application probe RTTs, not raw radio latency, throughput or recovery
latency. Each cell was an independent exact-page transfer with exit status zero.

For the outage, only the private hostapd process was stopped. After a ten-second
wait and negative probes, G4 reported not connected and carrier zero; AP ping
was 0/2. Hostapd was restarted, then the first ping check after an eight-second
wait passed 2/2. Neither Hopspot was restarted. This short outage does not prove
stale-peer expiry or survival of an in-flight session.

For the address change, the private DHCP server was restarted with a reservation
at 172.23.73.119 instead of the original .110 lease, and udhcpc was explicitly
invoked again. The old address disappeared, .119/24 appeared, and both routed
transfers still passed. This is same-subnet forced reacquisition, not unattended
lease renewal or a new-subnet transition. Auto-WiFi used IPv6 link-local peers,
so the unchanged link-local addresses are a material limit of this experiment.

A 20-second Wi-Fi capture contained eight UDP 42671 datagrams, alternating
194/226-byte payloads from each node at roughly ten-second intervals, with zero
capture drops. It ran after the initial page probes and supports announcement
cadence, not a packet-level reconstruction of the page sessions.

## Verification and remaining work

Host checks passed: three library tests, two binary tests, the two-hop HaLoW
page/relay test, two lifecycle tests, and the WebSocket page/capacity-recovery
test. Default/no-feature, Auto-WiFi-only and combined-feature builds checked;
combined-feature binary Clippy passed with warnings denied. Changed-source
formatting, diff whitespace and the validation registry passed. Whole-crate
format checking still reports an existing leading blank line in the untouched
`src/halow/tests.rs`. Socket-binding tests were rerun with the required local
socket access after the sandbox blocked startup.

The initial build exhausted disk space in the task's temporary compilation cache.
Regenerable cache files were cleaned, retaining the native probe, then the native
and static MIPS builds completed. The MIPS linker emitted its existing missing
prebuilt sysroot-library-directory warning; the build uses `build-std`, and the
result passed static ELF checks and executed on both boards.

DNS-SD-only peers with UDP discovery unavailable, long absence/peer expiry,
changed link-local addresses, memory/CPU soak and concurrent native HaLoW remain
separate qualifications. This run establishes short functional routed recovery,
not those broader guarantees.

## Process continuity and restoration

G4 PID 4681 / start tick 6998917 and Heltec PID 17538 / start tick 7255181
remained unchanged through the measurements. Each process retained its original
node-page identity. Final MemAvailable samples were 17,732 KiB on the G4 and
8,612 KiB on the Heltec, with no swap; these include other board workloads and
RAM-backed staging, and are not peak or per-process memory limits.

Both lab stages stopped through the rollback function. Saved network, wireless,
firewall and DHCP hashes matched on both boards. Original addresses, routes and
radio roles returned; pending UCI changes were empty and no lab interfaces,
firewall rules or processes remained. Stopped candidate executables were removed
from tmpfs, preserving identities and evidence. Board wall clocks were
unsynchronized; this report uses the host session date.

The pre-existing G4 Hopspot subsequently served its exact 3,542-byte page again
(55 ms probe RTT). The candidate was not installed persistently.

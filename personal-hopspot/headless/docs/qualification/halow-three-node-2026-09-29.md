# Native HaLoW three-node functional qualification

The G4 and two Heltecs passed eleven end-to-end page transfers on an open,
fully connected 802.11s mesh. This qualifies basic native discovery, shared
announcements, directed Reticulum traffic, concurrent transfers, and application
restart recovery. It does not qualify sustained throughput, field range, or a
forced two-radio-hop route.

Tested source: `0966fe6b96dc4fe6cb2c67c39e38c823bed3f5c6`, clean at build.
Application SHA-256:
`3cb73d0896de9c9f9c864d0e282d2accecaa279c2ae63ac0905521afd6dc21b2`.
The [structured evidence](halow-three-node-2026-09-29.json) includes per-transfer
results, capture counts/hashes, build identity, and recovery checks. Raw captures
remain in the development machine's private `/private/tmp/halow-three-evidence`
directory; they are not a durable public artifact or required to run the application.

## Conditions and method

All three radios reported two established mesh peers. Morse channel reporting
confirmed 924 MHz center and 8 MHz width; each peer's rate table selected MCS2,
long GI. Mesh forwarding and power saving were disabled, and transmit power was
set to 18 dBm. These were close-range bench tests. Vendor LED/button polling
continued normally: this was a functional check, not the previous optimized
raw-radio throughput experiment.

Each board ran the same hash-verified application from RAM with a fresh private
state directory, a distinct stable radio scope, four-peer admission limit,
ten-second announcements, and a bounded lifetime. Peer idle expiry was therefore
30 seconds with five-second housekeeping. No firmware or permanent application
installation was performed.

The laptop connected to one board's wired TCP interface and requested another
board's destination. Only that entry board was attached to the laptop probe.
The application path therefore crossed TCP and native HaLoW; the boards did not
use TCP to contact one another. IPv4 accessed the G4/Heltec A and scoped IPv6
accessed Heltec B. Each probe established a Reticulum link and compared all 3542
response bytes with the shared Hopspot page, exceeding a single radio frame.

## Transfers

The initial round used three concurrent transfers, then three concurrent reverse
transfers. These RTTs are application request measurements including the wired
leg, scheduling, and cryptography, not raw radio latency or throughput.

| Source board | Destination board | Request RTT | Result |
| --- | --- | ---: | --- |
| G4 | Heltec A | 141 ms | Exact page |
| Heltec A | Heltec B | 71 ms | Exact page |
| Heltec B | G4 | 119 ms | Exact page |
| G4 | Heltec B | 115 ms | Exact page |
| Heltec B | Heltec A | 91 ms | Exact page |
| Heltec A | G4 | 94 ms | Exact page |

Heltec B's application was then stopped gracefully for 62.49 seconds, beyond the
configured idle window. Its radio remained associated. During the absence,
G4 → Heltec A still passed (60 ms). Restarting with the same state and scope
preserved its destination; the runtime reported restoring four routes, four
destination identities, and one ratchet. All four transfer directions involving
Heltec B then passed concurrently, at 120, 130, 132, and 161 ms. This is an
application departure/rejoin check, not a physical unplug or radio-reset test;
peer expiry was inferred from its configured deadline, not independently traced.

Sampled application RSS was 3772 KiB on the G4, 3784 KiB on Heltec A, and
3756 KiB on Heltec B. These are individual samples after the initial transfers,
not peak memory limits or a soak measurement.

## Ethernet capture findings

Simultaneous captures selected only EtherType `0x88b6` on each radio interface.
The versioned envelopes decoded without truncated captured frames. All three
capture processes reported zero kernel capture-buffer drops.

| Sender | Outgoing broadcast frames | Unique broadcast payloads | Outgoing unicast frames |
| --- | ---: | ---: | ---: |
| G4 | 36 | 36 | 140 |
| Heltec A | 40 | 40 | 145 |
| Heltec B | 28 | 28 | 117 |

Every outgoing group frame was an announce. Link requests, proofs, and data were
unicast. Each group payload appeared once in its sender's Ethernet capture,
consistent with one shared-channel send rather than one send per known peer.
Matching payload hashes establish reception at both neighbors for most frames.
This does not measure firmware retransmissions or exact over-air transmissions.

Captures began and ended at different times. Restricting each sender's sequence
to the first/last payload observed by both receiving captures leaves 100 group
frames, with 199 of 200 expected neighbor observations. One G4 frame was absent
from Heltec B's capture inside that overlap. The available evidence cannot
separate RF/driver loss from every other pre-capture cause. Do not describe this
as lossless broadcast or turn the small sample into a field loss-rate estimate.

There were also 300 outgoing unicast announces (105/111/84 by sender), all with
nonzero hop counts and ordinary announce context. Most used hop count one;
nine from Heltec B used two. This is material rebroadcast overhead. The current
runtime honors recipient exclusions through directed delivery, so shared local
announces do not make all relayed announces physical broadcasts. Quantifying and
improving this behavior needs a separate announce-policy experiment with loop
suppression and directed-response semantics preserved.

## Recovery and reproducibility

Before mutation, fresh wireless/mesh11sd files and fixed-rate driver parameters
were saved privately on each board. Each board had an independent timed rollback.
After testing, temporary applications received graceful shutdown, flushed retained
state, and stopped. Original configuration files and every saved driver parameter
matched byte-for-byte; UCI pending wireless/mesh edits were empty. The original
AP/station modes returned and all three Morse health checks passed. The pre-existing
G4 TCP Hopspot remained running and its page still verified (50 ms).

For another run, build the committed source with `build.hopspot.g4 --with-probe`,
verify hashes on each board, establish independent wired management and fresh
rollback snapshots, and apply a locally legal matching radio profile. Start one
bounded host per board and filtered captures, run the six wired-entry/remote-page
combinations, perform a bounded application departure/rejoin, then restore and
compare all saved settings. Keep state directories private. Record the actual
Morse channel/rate tables: generic `iw` frequency/rate labels on these vendor
images represent a compatibility mapping, not the real S1G frequency or bitrate.

The subsequent [relay-broadcast qualification](halow-relay-broadcast-2026-09-29.md)
changes ordinary relay policy and verifies group egress on all three radios.

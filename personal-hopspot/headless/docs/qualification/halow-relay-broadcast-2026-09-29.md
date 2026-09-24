# HaLoW relayed-announce broadcast qualification

Ordinary relayed announces now select the shared HaLoW broadcast channel, just
like local announces. Directed path responses retain `Only(peer)` delivery.
Explicit `AllExcept(peer)` semantics remain unchanged. Core deduplication handles
previous-hop reception; the adapter adds no second deduplication mechanism.

This follows the [three-node baseline](halow-three-node-2026-09-29.md), which
exposed ordinary relays using unicast. The G4 and both Heltecs ran a fresh MIPS
build of the same source plus this core policy change. See the
[structured evidence](halow-relay-broadcast-2026-09-29.json) for the base commit,
dirty-build metadata, patch hash, binary hash, and individual probe results.
Private raw captures are in `/private/tmp/halow-relay-evidence` on the lab host.

## Results

All six directed page transfers passed, in two rounds of three concurrent probes.
Each returned the exact 3542-byte page across wired TCP entry and native HaLoW.
Request RTTs were G4→A 126 ms, A→B 180 ms, B→G4 208 ms, G4→B 90 ms,
B→A 84 ms, and A→G4 119 ms. B→G4 took 8.534 seconds overall, including
discovery/link setup; request RTT alone does not describe startup latency.

| Sender | Local broadcast announces | Relayed broadcast announces | Unicast announces | Other unicast frames |
| --- | ---: | ---: | ---: | ---: |
| G4 | 8 | 21 | 0 | 17 |
| Heltec A | 6 | 18 | 0 | 27 |
| Heltec B | 8 | 22 | 0 | 17 |

All 83 outgoing group frames were announces; 61 had nonzero hop counts.
Link requests, proofs, and data were unicast. No ordinary relay was observed
using unicast. Captures spanned approximately 36 seconds and each reported zero
kernel capture-buffer drops. Some relay payloads appeared twice, consistent with
the core's existing bounded announce retry behavior; this is not a promise of
exactly one send over an announce's lifetime. It is one shared-channel send per
egress operation, rather than one copy per peer. No payload appeared more than
twice in a sender's group capture. Directed path responses were covered by the
core selector regression, but were not separately induced in this radio sample.

The test retained the baseline profile: open 802.11s, 924 MHz/8 MHz, MCS2 long GI,
18 dBm, power saving off, mesh forwarding off, ten-second periodic announces,
and stock vendor polling. Each radio reported two established peers and healthy
Morse status. This is a close-range functional check, not a range, throughput,
long-running loop-suppression, or forced multi-radio-hop qualification. Hop counts
alone do not demonstrate a topology that requires two radio hops. Ethernet
captures do not prove exact RF transmission counts or lossless reception.

## Regression coverage

The new scheduled-relay test feeds a real reference announce through the engine,
checks one `All` HaLoW fleet directive, then returns an onward echo of the emitted
wire frame. The echo is ignored, the route count stays one, and the pending
retry is retired; advancing time emits nothing further. A selector regression
preserves directed responses. The 42 deadline tests, 15 node-egress tests, and
runtime shared-channel/restricted-target test passed. Validation registry checks
also passed; registry verification does not execute all registered suites.

## Recovery

Independent rollback timers were armed before changing any radio. After capture,
the temporary applications stopped gracefully. Wireless and mesh configuration
files and every saved driver parameter matched the snapshots byte-for-byte; no
pending wireless/mesh UCI edits remained. Original AP/station roles returned and
all three Morse health checks passed. The pre-existing G4 TCP Hopspot still
returned the exact page (48 ms request RTT).

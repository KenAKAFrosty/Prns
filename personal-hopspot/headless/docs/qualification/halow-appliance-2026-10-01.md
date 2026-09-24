# Three-appliance HaLoW qualification — 2026-10-01

Initial operation passed on one ThinkNode G4 and two Heltec HT-HD01-V2 boards.
The radio lifecycle check exposed an unresolved control-path recovery problem.
This is a bounded desk qualification, not appliance release approval. All vendor
configuration, module parameters, radio roles and management paths were restored.

[Structured evidence](halow-appliance-2026-10-01.json) records the build, metrics,
captures, outcome distinctions and limitations. Private local raw evidence is in
`/private/tmp/halow-qualification-20261001`; configuration backups, SSH credentials
and private controller identities are deliberately excluded from shared output.

## Setup and artifact

The simulator work was committed as `e852f85b9bcfbd7d41e19fec2120b65bb2c2fd0f`.
The initial clean build used `wifi-halow`, the qualified Rust
`1.98.0-nightly (6bdf43094 2026-06-01)` from `nightly-2026-06-02`, and Zig 0.15.2.
The moving `nightly` alias had advanced, so the build task now defaults to that
dated toolchain while retaining its exact compiler-commit check.

The first artifact was 3,612,712 bytes. Qualification then added the explicit
`--tcp-mode gateway` composition option. The tested Gateway candidate was
3,620,680 bytes, static MIPS32r2 little-endian O32 soft-float, SHA-256
`a7e425c20ad49e61c45a66855c64b86db62b520b9d9664b07e4d8cb260a16da0`.
Its build metadata marks the tree dirty; the structured record identifies the
modified source files by hash. The same executable was verified on all boards.

Each device received a private configuration backup, an independent rollback
timer and a RAM-only service. Wired management stayed independent; the Mac's
default internet route remained on Wi-Fi `en1`. No firmware image, calibration,
bootloader or persistent service was written. Seven inactive old lab executables
on the first Heltec were removed only after verifying matching retained local
copies; prior identities, configuration and logs were preserved.

The three radios used US profiles, open 802.11s, 924 MHz center, 8 MHz width,
MCS2, long guard interval, power saving off, mesh forwarding off and 18 dBm.
Each reported two established mesh neighbors and healthy Morse firmware.
Vendor LED/button polling stayed active. Synthetic VHT labels from `iw` are not
actual HaLoW PHY rates; Morse tooling verified center frequency and width.

The existing lab controller was provisioned with explicit inspection, app-message
and watch grants. Its private keys stayed on the Mac and target public keys were
pinned. There was no application announcement ticker. Before the first explicit
announce, each EtherType-filtered capture contained only its 24-byte PCAP header.
This does not claim RF silence: beacons and other EtherTypes were excluded.

## Initial-operation checks

| Check | Observation |
| --- | --- |
| Local authenticated Describe/AnnounceSelf | All three passed |
| Remote cold endpoint discovery with PointToPoint TCP ingress | Target-path timeout; no matching radio discovery request observed |
| Remote discovery after selecting TCP Gateway | All three passed, with explicitly established PRNS neighbor lanes |
| Build, interface, config and peer snapshots over HaLoW | All three passed through another board's wired ingress |
| Inventory projection | One HaLoW supervisor; two source-MAC peer children and one broadcast child in its peer inventory |
| App probe | Exact version/operation bytes plus verified controller hash on all three |
| Authorized invalid app payload | App-level `ApplyFailed` |
| Unlisted controller request | Exchange timeout; independent healthy-pair page passed concurrently |
| Page/Resource directions | Six baseline directions and six Gateway directions passed; exact 3,542-byte response each |
| Concurrent workload | 100 rounds of three page requests plus ten overlapping build reads: 310 successful operations in 74.753 seconds |
| Request RTT in concurrent workload | 46 ms minimum, 99 ms median, 170 ms p95, 330 ms maximum |
| Authenticated interface watch during six additional pages | Admission and ten ordered resync invalidations; pages passed |
| Quiet authenticated watch | Initial resync and two heartbeats |

PointToPoint unknown-path forwarding was a configuration mismatch, not evidence
of broken radio delivery. The core routing policy intentionally requires recursive
discovery participation. The new typed Gateway option leaves the default unchanged
and uses the existing interface policy. A native real-TCP regression verifies the
PointToPoint timeout and Gateway discovery/link success without prior announcements.
Gateway does not grant authority or prove arbitrary multi-hop cold discovery.

The first watch probe incorrectly required a heartbeat during continuous topology
changes. The producer sends resyncs for those changes and heartbeats during idle
periods. The quiet follow-up passed that contract; no producer change was needed.
Similarly, the inventory probe was corrected to use the established supervisor /
peer projection rather than expecting child interfaces in the top-level inventory.

The unlisted request's timeout is consistent with silent admission rejection, but
the timeout alone does not identify denial. No on-wire denial response was added.

## Traffic and resource observations

| Board | Post-load RSS | High-water RSS | File descriptors before/after | Threads | State before/after |
| --- | ---: | ---: | --- | ---: | --- |
| G4 | 4,396 KiB | 4,740 KiB | 12 / 12 | 2 | 40 / 40 KiB |
| Heltec | 4,400 KiB | 4,728 KiB | 12 / 12 | 2 | 40 / 40 KiB |
| Second Heltec | 4,372 KiB | 4,744 KiB | 12 / 12 | 2 | 40 / 40 KiB |

RSS includes RAM-backed executable pages. These are measured snapshots, not a
worst-case RAM budget or proof of no long-term leak. All three firmware health
checks passed after the workload. The small page workload exercises concurrent
Resources and control, not saturated bulk Resource throughput.

The pre-lifecycle captures contained 2,310 / 2,383 / 2,264 Ethernet frames. Outgoing
ordinary announcements were 9 / 9 / 7; all used `ff:ff:ff:ff:ff:ff`, including
7 / 7 / 5 relayed announcements. Zero ordinary-announcement unicasts appeared.
Each board also transmitted one broadcast path-request control frame; this is
separate from the announce rule. Outgoing unicast counts were 1,150 / 1,166 /
1,122. These packet-socket captures do not expose RF retries, aggregation or airtime
and are not an exactly-once RF-delivery or loss-rate measurement.

Absent HaLoW signal/rate measurements remained `HaLow(Unavailable)` / JSON `null`
in public peer snapshots. Driver station information was recorded separately;
it was not substituted into unsupported runtime counters.

## Recovery finding

Wi-Fi lifecycle commands recreated the second Heltec's `wlan0`: its Linux index
changed from 10 to 11 while its MAC stayed unchanged. The radio rejoined both
mesh neighbors and passed firmware health. This was interface recreation, not a
qualified sustained RF partition: the first down command's inspection already
showed `wlan0` up, and continuous endpoint isolation was not established.

Wired control on the affected running process and three pages between the other
two boards passed. Over-air control to the affected process failed during link
establishment. Restarting only that application preserved the page and control
identities, but a subsequent request through G4 failed during target discovery.

A separate bounded follow-up restarted all three applications with retained state
and explicit announcements. All six page directions recovered, both other targets
passed full over-air snapshots/app messages, and every supervisor/peer ID matched
the pre-recreation inventory. Control to the second Heltec through G4 still timed
out at discovery, including after the successful page exchanges. No matching
endpoint request was found in the final filtered radio captures.

The evidence narrows the problem to lifecycle/control-route recovery; it does
not isolate the cause. An old packet-socket binding and retained negative/path
state need independently controlled regressions. Do not advertise automatic
recovery or recommend blindly deleting retained state. The immediate follow-on
should reproduce a TCP controller through a HaLoW router to a recreated target,
retain identities and authorization, and distinguish binding replacement from
cached-route refresh before repeating the desk test.

## Installation evidence and restoration

The candidate gzip is 1,501,636 bytes; two compressed slots total 3,003,272 bytes.
G4 RAM-only expansion produced the exact executable hash and `--version` succeeded.
Persistent compressed slots with verified RAM expansion are therefore a viable
design to qualify, not an installed or power-loss-safe updater. The
[deployment specification](../halow-deployment.md) defines the staged transaction,
controller enrollment, compatibility and remaining gates. Both appliances now
have development-guide entry points in the web install experience.

After both temporary trials, all five saved configuration files matched on every
board; module parameters matched and there were no pending UCI edits. Original
924 / 924 / 908 MHz roles and addresses returned, all Morse health checks passed,
all management HTTP pages responded, and both Heltec button/LED services remained
running. Temporary services were stopped. No reboot or persistent installer was
attempted.

## Software checks and scope

```sh
cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml \
  --features wifi-halow,websocket,wifi-auto
cargo clippy --locked --manifest-path personal-hopspot/headless/Cargo.toml \
  --features wifi-halow,websocket,wifi-auto --all-targets -- -D warnings
cargo check --locked --manifest-path docs/website/Cargo.toml --features web
./tools/prns run build.hopspot.g4 --zig "$ZIG" --output "$NEW_BUNDLE" --with-probe
```

Native tests, Clippy and website check passed on macOS arm64; the cross-build
passed with the known absent-prebuilt-target-library linker warning, and ran on
all three MIPS boards. The native Gateway test was also run separately. The HaLoW
simulation campaign was qualified before this turn and committed separately.
Physical forced multi-hop, field range, long soak, saturated encrypted throughput,
public signed downloads, durable boot/update recovery, other host platforms and
ESP-NOW remain outside these results.

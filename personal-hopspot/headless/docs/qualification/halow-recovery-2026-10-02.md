# HaLoW binding and routing recovery qualification

The G4 and both Heltec HT-HD01-V2 boards recovered authenticated control and all
six directed page paths after their HaLoW network devices were removed and
recreated. Hopspot kept running with the same configured interface identity.
Retained-state application restarts also recovered without re-enrolling keys or
grants. One Heltec page handshake timed out; an explicit retry passed without
another announce. This qualifies bounded recovery from the gaps recorded in the
[October 1 report](halow-appliance-2026-10-01.md), not lossless radio delivery or
persistent appliance installation.

The [structured results](halow-recovery-2026-10-02.json) contain executable hashes,
build inputs, measured process snapshots, capture hashes and verification limits.
Private evidence is retained locally in `/private/tmp/halow-recovery-20261002`;
configuration backups, access helpers and private identities are excluded from
the repository. The measured candidate was built from `c98f2e182` plus the
uncommitted recovery changes, not from the unchanged commit alone.

## Ownership and fixes

The Linux binding and routing failures needed separate fixes:

- `prns-ffi::ethernet` owns the validated name/index/MAC binding and bounded
  kernel link notifications. Down/delete invalidates an old binding even when a
  later up event appears in the same batch. Notification loss, truncation or
  malformed batches require a fresh binding. Subscribing before packet bind
  closes the setup notification window. This follows Linux's
  [netlink notification contract](https://www.kernel.org/doc/html/latest/userspace-api/netlink/intro.html).
- `HaLowDevice` keeps one configured supervisor while owned `HaLowRadioSource`
  generations come and go. Fatal receive releases the old socket and child
  lanes. Reopening uses the existing jittered reconnect policy: initially
  125–375 ms, capped at a 2.5–7.5 second delay; a stable 30-second connection
  resets the schedule. There is no periodic device/station polling while bound.
  `Connected` means an open packet binding, not RF association or reachability.
- Core discovery accepts an exact, verified cached path response for an
  outstanding request without making it a new announce observation or renewing
  its route age. Signature, retained content, live route, ingress, next hop and
  hop count must all match. Ordinary repeated announces still do not settle it.
- A configured gateway/recursive ingress can search again when a retained route
  is unresponsive or its receiving interface has retired. It cannot advertise
  the retired route as usable. Existing authority, loop prevention, deduplication,
  pending-request bounds and rate limits still apply; no state wipe is needed.
- Recursive path-request egress excludes the shared announce-only group lane.
  Discovery uses attached MAC peers. Announcements still use physical broadcast;
  normal control, link and Resource traffic stays directed.

No radio profile mutation, generic shell command, automatic application announce
or automatic replay of an application request was added to Remote Control.
Controller identities and target keys remain pinned. MAC identity is transport
addressing; authentication remains cryptographic.

## Deterministic model

The fixture now composes a controller, an actual Tokio TCP connection in Gateway
mode, and HaLoW targets with separate page and control endpoints. Only explicit
page announces seed MAC neighbors; the control endpoint is not preannounced.
Retained-restart cases use the real file store and snapshot decoders. Restored
nodes configure no new grants, so subsequent success depends on retained grants
and identities rather than fixture reprovisioning.

Ten added recovery scenarios cover cached repeated discovery, adapter replacement,
failed handshakes, a route through a retired peer, target/gateway/controller
retained restarts, and managed missing-device, coalesced down/up and fatal-receive
failures. The final scenario deliberately drops a page LinkRequest, confirms the
actual affected wire packet, observes the typed link timeout, keeps another
authenticated control operation progressing, and retries discovery/link/Resource
without another announce. A delayed request from an obsolete generation cannot
reach the app handler or settle a replacement waiter.

Managed cases perform eight binding cycles each, preserve the parent ID and
MAC-derived peer ID, bound reopen attempts, and require observed readiness within
8,000 simulated milliseconds. That fixture bound includes the reconnect plateau;
it is not an eight-second RF recovery promise. Every case compares two complete
fresh traces, observations and measurements, then requires actors, radios, queues,
pending deliveries and armed faults to drain.

Routine: **136 cases twice**, `routine/run-0011`. Extended: **1,088 cases twice**,
`extended/run-0006`. The expanded run took 218.63 seconds on macOS arm64. Artifact
roots and replay instructions are in the
[simulation qualification](../../../../validation/simulation/measurements/halow-qualification.md).
Standalone inputs are checked in for missing devices, retired routes and the
lost page handshake. The simulator does not emulate Linux packet sockets,
Morse firmware, RF modulation, hidden terminals or the MIPS performance profile.

## Physical procedure and observations

Temporary RAM services used one common 3,636,552-byte static MIPS32r2 little-endian
O32 soft-float executable. Kernel versions remained G4 5.15.150 and Heltec
5.15.167. Vendor drivers/firmware were retained. A separate 5,458,308-byte build
with Auto-WiFi and WebSockets passed cross-compilation/ABI checks but was not
deployed in this run; it does not establish an optional-feature storage budget.
Final review removed duplicate interface-index storage from the packet socket:
send and receive now derive it from the same validated binding. That final
3,636,456-byte candidate passed cross-compilation/ABI checks and was not deployed.
The physical table below therefore describes the preceding qualified binary,
whose hash is retained separately. The Linux module/test layout was also curated
after hardware qualification without changing the notification parser.

All three used the existing US lab profile: open 802.11s, 924 MHz center, 8 MHz,
MCS2, long guard interval, power saving off, mesh forwarding off, 18 dBm. Vendor
LED/button services remained active. Configuration backups and an independent
20-minute rollback lease preceded changes. Wired management stayed available.

For each board, the harness waited for actual `wlan0` disappearance after `wifi
down`; the vendor command can return before removal. Wired inventory showed the
same parent disconnected, and wired gateway inspection progressed. A control
attempt through the unavailable radio timed out, exercising negative route
evidence. After `wifi up`, the harness waited for two established mesh peers,
reapplied the explicit lab settings and rebound the capture socket too. It then
performed an authenticated app exchange and all six exact page transfers without
restarting Hopspot or announcing page/control endpoints again.

| Board | PID before/after | Interface index before → after | Parent interface ID | FDs before/after | RSS KiB before → after | Radio up through control and six pages |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| G4 | 22701 | 24 → 25 | `22f24af394977e2c` | 13 | 3956 → 3968 | 10.980 s |
| Heltec | 1180 | 92 → 93 | `226f1329ac9496ba` | 13 | 4120 → 4072 | 32.356 s, including failed page round and retry |
| Heltec 3 | 29825 | 21 → 22 | `2278fa3e676eb008` | 13 | 4140 → 4096 | 12.433 s |

These times include vendor radio setup, readiness checks, capture setup, control
and pages. They are not isolated packet-rebind latency measurements. Short RSS
and descriptor snapshots do not establish long-running memory stability.

The Heltec's first page round exposed a remaining delivery timeout. Its path
response was recorded at both the Heltec and G4. Two direct G4→Heltec page
LinkRequests appeared in the refreshed G4 capture and neither appeared in the
refreshed target capture. An explicit retry then passed all six page directions.
The failure snapshots were copied before capture stop statistics; final stopped
captures reported zero kernel drops. Capture clocks differ across boards, so
packet content, not absolute timestamp alignment, was used. This supports a
sender/receiver delivery-boundary finding, not a proven RF, firmware or kernel
root cause. No IQ/airtime experiment ran and no speculative driver fix was made.

Each application was then restarted against its existing state without controller
grant arguments. Verified-controller app messages and six pages passed after
every restart. Final over-air build, interface, configuration and peer snapshots
passed for each target. An authorized invalid app message returned `ApplyFailed`;
an unlisted controller timed out while an independent page completed. Timeout
alone does not identify denial. The simulator additionally verifies no Response
and no app admission before authorization.

Final filtered captures contained 356 / 310 / 245 PRNS Ethernet frames at the G4 /
Heltec / Heltec 3, all directed. This final phase did not issue announces, so it
does not independently demonstrate physical announcement fan-out. The retained
initial Heltec 3 capture recorded twelve group frames, all ordinary announces;
the deterministic recovery wire contract checks every group transmission and
rejects non-announce traffic. Captures are Ethernet observations, not RF retry
or airtime counts.

## Verification and restoration

| Check | Result / host |
| --- | --- |
| `cargo test --locked` | Passed on macOS arm64; core library 2,097 passed, 3 ignored |
| Cached-response matrix; path-request owner tests | Passed; 8 response variants and 42 path-request tests |
| Tokio HaLoW owner tests | Both passed on macOS arm64 |
| `cargo test --locked --manifest-path prns-runtime/impls/embassy/Cargo.toml --lib` | 174 passed on macOS arm64 |
| `cargo test --locked --manifest-path personal-hopspot/headless/Cargo.toml --all-features` | Passed on macOS arm64, including real TCP gateway, lifecycle and two-hop tests |
| Registered `halow-simulation` | 14 passed, 2 explicitly ignored; 136 cases twice |
| Expanded HaLoW campaign | 1,088 cases twice |
| Registered `virtual-device-simulation` | 449 passed, 13 explicitly ignored on macOS arm64; 209.862 seconds |
| Registered `halow-resource-stability` | 32 cycles; peak 4,531,776 bytes, retained 56 bytes, within harness budgets |
| Core, Tokio adapter and simulator Clippy | Passed on macOS arm64 with warnings denied |
| Linux FFI packet filtering and netlink parser tests | 4 passed on each real MIPS board |
| Minimal and optional-feature MIPS bundles | Static ABI/build checks passed; minimal candidate executed on all three boards |
| `cargo test --locked --manifest-path docs/website/Cargo.toml` | Passed on macOS arm64 |

The focused mutation command was `cargo mutants --no-config --file
prns-core/src/engine/node_ingress/announce_completion.rs --re apply_announce_ingest
--package prns-core --output /private/tmp/prns-halow-recovery-focused-mutation --
--lib`: baseline passed, one changed-owner mutant caught. An earlier configured
attempt selected additional unchanged wake-scheduling owners: 15 caught, two
missed. Those observations remain for later triage; they were not declared
equivalent or used to justify unrelated changes. Linux-target Clippy was not run
because the pinned target toolchain lacks that component; macOS Clippy does not
compile Linux-only FFI. Registry/format/diff checks passed except `./tools/prns
verify`, which still reports the three pre-existing misplaced network-lab scripts.

An initial unoptimized FFI test upload exceeded Heltec RAM staging space. It was
removed before radio configuration; the optimized 2,140,716-byte test executable
then ran successfully on all boards. A harness assumption that `wifi down` was
synchronous was replaced by observing actual device removal. Neither failure
required a board reboot or state wipe.

All five original configuration hashes, saved Morse module parameters and empty
pending UCI changes were verified after restoration. Firmware health passed on
all three, candidate/capture services stopped, and rollback timers were canceled
after verification. Original radio modes/channels returned: G4 AP at 924 MHz,
first Heltec station at 924 MHz, second Heltec AP at 908 MHz. No firmware flash,
board reboot or private-identity wipe occurred.

Persistent procd installation, interrupted activation/update, power loss, low
space, longer load and resource monitoring, physical forced multi-hop, field
range and region-specific profiles remain separate qualification gates. The
[deployment transaction](../halow-deployment.md) uses this recovery result as one
completed prerequisite rather than declaring a public installer ready.

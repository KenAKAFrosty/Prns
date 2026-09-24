# HaLoW software-transport qualification

This qualification exclusively targets the HaLoW software path. Four production
Tokio nodes use the production `HaLow` and managed `HaLowDevice`
supervisor, envelope parser, source-MAC peer identities, broadcast and unicast
pacing, crypto, routing, Resource transfer and Remote Control. Application state
is `()`. Controller identities and target keys are explicitly provisioned in the
fixture; announcements are explicit actions. No physical devices are accessed.

The recovery follow-up composes wired Gateway ingress with radio discovery and
real retained state. Its separately bounded
[three-board qualification](../../../personal-hopspot/headless/docs/qualification/halow-recovery-2026-10-02.md)
checks the Linux/Morse boundary which this simulator deliberately does not model.

## Medium and scheduling

`src/halow` owns a source/destination-aware datagram medium. Broadcast is one
accepted transmission delivered to reachable neighbors; unicast selects one MAC
and never falls back to broadcast. Directed paths permit asymmetric connectivity.
Kernel acceptance and receiver delivery remain distinct. A directed path can also
permit only broadcast reception, independently of accepted unicast sends. `VirtualHaLowRadio`
implements the production cancel-safe datagram trait. Stalled or failed sends,
fatal receive errors, first-frame reception and queue pressure reach the actual
supervisor and peer tasks.

The existing manual driver coordinates medium delivery and Tokio timers.
Seeded cyclic actor ordering, fixed validation-only entropy streams and a fixed
logical boot epoch make complete traces comparable across fresh fixtures. Crypto
executes inline. A new clock variant identifies HaLoW advancement errors by
their owner. No native worker/thread interleaving or firmware timing is modeled.

Every attachment gets a monotonic radio ID. Pending delivery captures the
recipient generation, so a delayed packet cannot reach a replacement using the
same MAC. Reachability is sampled at transmission; already scheduled packets
remain in flight. Delay, loss and duplicate rules apply to explicit next matching
datagrams. Protocol workloads independently parse the struck packet and verify
its actual request, response, advertisement or Resource-part boundary.

| Owner | Explicit budget |
| --- | ---: |
| Live medium radios | 4 |
| Receive datagrams per radio | 256 |
| Pending medium deliveries | 512 |
| Armed faults | 16 |
| Retained medium events | 65,536 |
| Registered actors | 24 |
| Actor polls per settlement | 32,768 |
| App invocations | 256 |
| Announce observations | 4,096 |
| Production peers per supervisor | 16 |
| Production queued datagrams per peer | 16 |
| Production peer idle interval | 10 seconds; checked every 5 seconds |
| Control response deadline | 100 ms |
| Dense Resource response | 4,096 bytes; 30-second fault recovery budget |

Medium queue/pending capacity losses are recorded and fail full-node qualification.
The intentional pressure case fills the separate production peer lane. Its
128 one-byte invalid RNS frames have valid HaLoW envelopes: the test starts a
healthy controller request while they are queued and measures actual peer RX-byte
growth at a settled boundary. In the final routine profiles, 70 frames reached
that lane, with the remaining 58 dropped before delivery to it; the healthy
request and subsequent flooded-peer traffic passed. It does not invent a driver
drop counter. Trace overflow
retains the bounded prefix and reports omitted events; a qualified run requires
complete retention. Teardown remains usable even after trace-budget exhaustion.

## Contracts exercised

- Shared-medium discovery and a forced chain run real encrypted links and exact
  multi-frame Resource responses. Immediate peer identity comes from the relay
  MAC and local scope, rather than the remote destination. Removing the relay
  prevents learned routes from bypassing it; restoring it recovers traffic.
- An established asymmetric path loses requests while an independent controller
  continues. Restoring the missing direction recovers the original link.
- Local and ordinary relayed announces use broadcast. Non-announce packets remain
  unicast. Broadcast duplication and previous-hop echoes terminate within the
  production announce retry/jitter bounds, without adapter deduplication or
  automatic announcements.
- Control requests and responses tolerate delay/duplication; loss and delivery
  beyond the request deadline produce typed local timeouts. A verified late
  request can still execute. Late replies cannot settle another waiter. Concurrent
  traffic retains the correct controller identity and response ownership.
- Lost Resource advertisements and data parts recover. Later parts overtake a
  delayed first part, a subsequent part is duplicated, and the real receiver still
  returns all 4,096 expected bytes. Another controller progresses concurrently.
- Canceling a local caller does not undo app admission. Its delayed orphan reply
  cannot capture a replacement waiter on the same link.
- Stalled/failed sends preserve destination and allow independent-neighbor
  progress. The actual two-second send deadline releases stalled transport work.
  Idle expiry/re-admission preserves the MAC-derived interface ID.
- Delayed traffic for a retired adapter cannot complete on its replacement.
  Fatal receive failure detaches the supervisor and children. Both explicit
  adapter replacement and managed reopening recover after observed readiness.
- Real Remote Control build/interface/config/peer inspection and maximum-size
  96-byte app messages run over HaLoW alongside Resources. At the production
  16-peer cap, all peer rows and the shared broadcast child page in ID order;
  unavailable radio/rate measurements remain unavailable. Unlisted controllers
  receive no Response packet and never reach the app handler.

Transport MACs are not authorization credentials. A valid envelope from an
untrusted source can consume a bounded peer slot before Reticulum authentication.
The cap rejects additional sources without evicting existing peers. This is the
current admission contract, not a claim of Sybil resistance.

## Replay and resource evidence

Version 1 cases specify seed, topology and one atomic qualification scenario.
Unknown fields, versions and unsupported scenario/topology combinations are
rejected. Each scenario owns its prerequisites and recovery; there is no generic
application policy or production development registry.

Routine inputs use seeds `0`, `1`, `42`, `0x5eed`: 144 cases, each run twice.
Extended inputs use seeds `0..31`: 1,152 cases, each run twice. Comparison includes
every actual datagram byte, source/destination, scheduled copy, delivery outcome,
radio generation, app invocation, announce observation, semantic operation
counter and pressure measurement. Wall-time measurements are excluded.

Every case retains structured JSON for both fresh runs. Failed invariants and
replay mismatches retain their input and complete available trace; incomplete
retention is marked explicitly. Cases are atomic probes, so this slice does not
add a sequence reducer. Artifact roots are
`validation-artifacts/results/halow-simulation/{routine,extended,replay}/run-NNNN/`.
Extract an artifact's `case` object for standalone replay:

```console
./tools/prns run repo.simulation.halow.replay -- --case validation/simulation/cases/halow/chain.json
./tools/prns run repo.simulation.halow.replay -- --case validation/simulation/cases/halow/recovery-retired-page.json
./tools/prns run repo.simulation.halow.replay -- --case validation/simulation/cases/halow/recovery-link-loss.json
./tools/prns run repo.simulation.halow.replay -- --case validation/simulation/cases/halow/recovery-delayed-radio-boot.json
./tools/prns run repo.simulation.halow.replay -- --case validation/simulation/cases/halow/recovery-missing-unicast-paths.json
python3 validation/run.py run --suite halow-simulation
python3 validation/run.py run --suite halow-simulation-extended
python3 validation/run.py run --suite halow-resource-stability
```

Runtime Resource rows, pending admission and any crypto ownership must drain
before shutdown. Shutdown requires all actors and medium radios, receive queues,
pending deliveries and armed faults to release. A separate isolated heap process
runs 32 construction/work/pressure/replacement/restart/drop cycles; its measured
retention budget is 64 KiB and peak budget is 32 MiB. These are harness bounds,
not a shipping-device footprint.

The initial chain broadcast probe caught a fixture horizon 24 ms short of the
last permitted retry; its boundary now derives from the production grace/jitter
constants. Reconnection probes wait for actual source-peer RX progress, since
announce pacing can defer transmission and core deduplication may suppress a
repeated announce event. Quiet-peer expiry requires disabling successful TX as
well as incoming traffic: active-link keepalives otherwise correctly refresh it.
Those initial fixes were fixture assumptions; the later recovery work below
also changed the production owners and has its own evidence.

## Gateway and device recovery follow-up

Twelve recovery scenarios add a real Tokio TCP Gateway connection, independent page
and control endpoints, repeated cached discovery, failed handshakes, a retired
page route, target/gateway/controller retained restarts, and managed missing-device,
coalesced down/up and fatal-receive faults. Retained cases use the actual file
store and snapshot decoders. On restart, the recipe provides no new grants:
authenticated success depends on restored authority and pinned identities.
Page announces seed neighbors explicitly; no control preannounce is required.

The delayed-radio-boot case starts the actual managed source with no device.
Two wired boot windows, each beyond the former five-second readiness deadline,
permit authenticated app messages and the exact page. A restart reloads retained
grants rather than enrolling the controller again. The later device appearance
binds within eight simulated seconds; moving wired ingress to another gateway
then recovers the page and authenticated control over HaLoW. Missing-device retry
counts are bounded, a healthy binding makes no further open attempts, and all
ownership drains at shutdown. This is software readiness/recovery coverage; it
does not simulate OpenWrt UCI commits, boot scripts, PHY rates or power loss.

On 2026-10-02 the updated routine campaign passed all 144 cases twice in
`routine/run-0014`, and the separate recovery matrix replayed twelve cases across
four seeds twice. The full HaLoW integration test passed fourteen tests with two
explicit ignores in 43.25 seconds on macOS arm64. All eight medium owner tests
and simulator Clippy passed. The expanded 1,152-case campaign has not been rerun
for these additions; the earlier 1,088-case result belongs to the previous
scenario set.

The missing-unicast-paths case permits group reception and kernel send acceptance
while dropping directed frames. Page announcements reach the gateway, but a
Remote Control path request must fail with its typed timeout. Direct wired
control and the exact page still work. Restoring directed delivery recovers
authenticated control and the page without restarting the target. This models
the observed delivery boundary, not Linux HWMP or Morse firmware internals. The
fixture gives different physical wired gateways different connection identities;
reusing one label had incorrectly aliased retained page paths when moving entry.
Production TCP clients already derive those identities from the actual address.

The retired-page case first routes through a relay, replaces the gateway binding,
admits a different MAC without refreshing routes, and fetches the exact page by
rediscovering across attached peers. The link-loss case confirms that the armed
fault hit an actual page LinkRequest, waits for its typed link timeout, observes
independent authenticated control progress, then explicitly repeats path/link/
Resource without another announce. A delayed obsolete-generation request cannot
reach the app handler or complete a new waiter.

Each managed case performs eight cycles. It verifies stable parent and peer IDs,
old owner release, bounded reopen attempts, wired progress while the device is
missing, and actual binding readiness within 8,000 simulated milliseconds.
The existing reconnect policy has a 2.5–7.5 second jittered plateau. The fixture
readiness deadline is not an RF association or operation-completion guarantee.
Every recovery trace checks that group transmissions contain only announcements;
recursive path requests use peer lanes. The unchanged unlisted-controller probes
still require no Response and no app admission.

Production fixes are owned separately: Linux kernel binding invalidation in
`prns-ffi::ethernet`, bounded reopening in the Tokio HaLoW supervisor, and
unavailable-route/exact-cached-response handling in core discovery. The cached
response matrix preserves the entire retained route row and suppresses ordinary
announce callbacks. Gateway rediscovery retains authorization, request bounds,
deduplication and rate limits. See the hardware report for the owning tests and
focused mutation findings, including two off-scope missed mutants requiring
later triage.

Earlier recovery artifacts are `routine/run-0011` (136 cases); expanded artifacts
are `extended/run-0006` (1,088 cases), each with two complete fresh runs. The
registered routine suite passed 14 tests with two explicit ignores. The expanded
campaign passed in 218.63 seconds on macOS arm64. The 32-cycle isolated heap probe
passed with peak 4,531,776 bytes and retained 56 bytes; its budgets are unchanged.
Simulator, core and Tokio Clippy passed with warnings denied. These results do
not imply Linux packet sockets were executed by the macOS simulator.

## Remaining hardware boundary

The medium models Ethernet datagrams around the configured radio. It does not
model modulation, sensitivity, airtime, collisions, hidden terminals, Linux
AF_PACKET or bridge behavior, Morse aggregation/retry behavior, RF range or
performance on the MIPS CPU. The existing board captures cover those adapter
boundaries separately, with their recorded limits.

Linux packet-socket rebinding is now automatic and its bounded lifecycle has
separate physical evidence on the G4 and both Heltecs. The simulator controls
owned binding generations; actual netlink, device ioctls and AF_PACKET behavior
remain outside its medium. Production install/update orchestration, electrical
power-loss recovery and long-running device resource limits still need desk
qualification. A software chain does not establish a forced physical RF chain.

The [persistent mesh boot follow-up](../../../personal-hopspot/headless/docs/qualification/halow-mesh-boot-2026-10-02.md)
qualified the explicit US radio profile and controller-seeded over-air checks on
all three boards. It found missing kernel unicast paths despite group reception,
which the new delivery scenario captures without claiming to simulate HWMP.
The independent lab rollback restored saved files, but mesh-to-AP transitions on
the G4 and second Heltec needed clean vendor reboots for operational recovery. Electrical power cuts and
the public radio transaction remain outside this qualification.

## Initial verification before recovery follow-up

Initial results on macOS arm64, retained as historical evidence:

| Check | Result |
| --- | --- |
| `cargo test --locked -p prns-simulation --features controlled-time --lib halow` | 7 owner tests passed |
| Registered `halow-simulation` | 12 tests passed; 96 routine cases passed twice |
| Registered `halow-simulation-extended` | 768 cases passed twice |
| Registered `halow-resource-stability` | 32 cycles passed; peak 4,528,256 bytes; retained 56 bytes |
| Registered `virtual-device-simulation` | 447 passed, 13 explicitly ignored; 178.539 seconds |
| Tokio interface `--features wifi-halow --lib wifi_halow` | Both existing adapter tests passed |
| Headless `--features wifi-halow --test halow_multihop` | Existing two-hop test passed |
| Simulator Clippy `--features controlled-time,heap-profile --all-targets -- -D warnings` | Passed |
| Ordinary `cargo check --locked -p prns-simulation` | Passed |
| Named HaLoW standalone replay task | Chain case passed twice |
| Registry verification, formatting and diff checks | Passed |

Initial routine artifacts are `routine/run-0006` (96 files), expanded artifacts
are `extended/run-0002` (768 files), and standalone chain replay is
`replay/run-0001`. Every campaign file contains both traces and public
observations. Earlier runs and initial failing fixture evidence remain separate.
The focused suites also run under the normal verification registry.

`./tools/prns verify` still fails on the pre-existing placement of
`personal-hopspot/headless/scripts/network-lab/{lease.sh,prepare.py,radio.sh}`.
Those unrelated scripts were not moved. The replay task itself works. The initial
simulation-only work did not change a shipping mutation surface or execute
physical devices. The recovery follow-up did both, as recorded separately above
and in the three-board report. Other operating systems remain unrun.

# Virtual device simulation

This non-shipping crate drives the production Personal Reticulum runtime through
host-native models of the hardware and media around it. It does not reimplement
Reticulum behavior.

The foundation provides a deterministic, bounded frame medium and a production
`Interface` adapter. Capstones run real `PrnsNode`s through announce, link
establishment, and request/response without sockets or physical hardware,
including a 128-node sparse ring on a manually controlled runtime clock.

The [current roadmap and replay-input audit](replay-roadmap.md) separates proven
capabilities from the next milestones. Forward work prioritizes shared behavioral
contracts across Tokio, Embassy and mixed configurations, then bounded generated
scenarios, persistence/reboot faults, representative scale/churn and selected ISA
integration. The [first shared contract](measurements/runtime-contracts.md) uses
one scenario and independent expected results for all four runtime pairings.
The [generated campaign](measurements/generated-runtime-contracts.md) composes
three fault/recovery operations in every order across those same pairings.
The [power-loss foundation](measurements/journal-power-loss.md) now exercises the
production flash journal through torn writes, partial erases and byte-image reboot.
The [embedded restore follow-up](measurements/embedded-restore-cuts.md) additionally
checks decoded group settings and logical time through the real persistence owner.
The [queued-owner campaign](measurements/queued-persistence-cuts.md) adds torn
append and compaction recovery through real admission and completion handling.
The [grant-storage campaign](measurements/grant-persistence-cuts.md) checks whole
authorization-table recovery across interrupted permission changes and revocation.
The [asynchronous core qualification](measurements/core-work-qualification.md)
adds controlled crypto completion, overlapping three-node traffic, public-outcome
attribution, replay/reduction and isolated resource stability checks.
The [Remote Control campaign](measurements/remote-control.md) runs real Tokio
nodes with fixed entropy and manual time through authenticated inspection, app
messages, concurrent authorization boundaries and bounded interface watches.
The [HaLoW qualification](measurements/halow-qualification.md) runs the native
supervisor through source-MAC datagrams, broadcast echoes, forced relay paths,
delivery faults, peer pressure, adapter replacement and concurrent Remote Control.

The medium intentionally makes its limits and faults explicit:

- endpoint, receive-queue, and trace capacities are required configuration;
- channel tags are nonempty, bounded, and unique within a medium;
- scheduled frame drops use stable transmission ordinals;
- a logical medium clock drives bounded delay, duplication, and reordering;
- versioned seeded recipes materialize exact, replayable fault plans;
- every accepted transmission and delivery outcome enters a bounded trace;
- trace eviction is counted instead of silently pretending the trace is whole.

The BLE lab adds bounded discovery, per-radio connection budgets, control/data
queues, and explicit link loss. Connection admission requires a powered dialer
and a powered, advertising listener. Queued and established connections share
the budget; closing either endpoint releases its reservation. The BLE capstone
runs two production nodes and exchanges requests before loss, after forced
disconnect, and after radio disable/re-enable.

Backend limits explicitly separate nonzero inbound-link, connection, and
discovered-peer capacities. Discovery retains at most the configured number of
addresses, evicting the least recently observed peer; repeated observations
refresh recency, but dialing does not. Snapshots expose the bounded history in
observation order and a saturating lifetime eviction count. Evicting a sighting
does not close an established link. This is a simulator retention policy, not
an emulation of a particular operating system's scan cache.

A sighting records the attached radio instance as well as its address. A cached
or queued sighting of a departed backend cannot admit a connection to a new
backend that reuses that address; the replacement must be observed first.
Turning a radio off clears both its discovery history and queued observations,
without resetting eviction statistics. Stopping scanning alone retains history.

Control messages cross bounded characteristic queues as encoded bytes and use
the production parser on receipt. Data uses the production GATT fragmenter and
reassembler, with explicit limits on complete characteristic values and queued
fragments. Completed frames use the same core whole-frame copy check as native
BLE sources; the simulator only maps the typed error into its own diagnostics.
Each peer's smaller value limit applies to the connection, and the
production BLE frame ceiling bounds reassembly. Tests cover exact fragment bytes,
maximum frames, cancellation, backpressure, malformed control values, and
disconnect during a fragmented send. The full-node capstone transfers 256-byte
requests through 20-byte data values before and after recovery.

Tokio BLE peer receive storage is bounded by the packet MTU plus maximum
interface-authentication headroom, rather than the global frame ceiling.
Source adapters refuse insufficient buffers without returning a truncated
prefix, and peer tasks check returned lengths before using them. A
[local future-layout measurement](measurements/ble-peer-buffer.md) records the
allocation change and its limits; it is not a total-memory or fleet-capacity
claim.

The logical medium clock owns delivery and advertisement scheduling. Tokio BLE
supervisor cooldowns, handshake timeouts, and recent-member status grace use
Tokio time, matching the manifold's monotonic clock. The shared BLE policy owns
the 10-second greeting budget used by both Tokio and Embassy; adapters own the
clock and timeout mechanism. Tests can pause the Tokio clock
and advance directly to a deadline without a wall-time wait. Test-only Tokio
clock controls are not enabled by this package's normal dependency features.

The `ble_timing` regression drives the production supervisor against a virtual
remote through `Fleet::detached`, not a full node. A group mismatch at a nonzero
runtime instant blocks redial until exactly 60 seconds later; a silent handshake
holds its slot until 10 seconds, then releases it for a new connection. The
owner's status test checks the 3-second grace boundary. Single explicit polls
keep negative assertions from accidentally triggering Tokio's automatic time
advance. The cooldown regression fails against the previous wall-time policy
clock. The focused commands are:

```console
cargo test --locked -p prns-simulation --test ble_timing
cargo test --locked --manifest-path prns-interfaces/impls/tokio/Cargo.toml --features bluetooth-auto-runtime --lib bluetooth_auto
```

The `ble_timing` tests advance medium ticks and runtime time independently; the
opt-in manual bridge below coordinates them for hand-polled scenarios. Full runtime
replay is not yet deterministic, and wall-clock boot timestamps and OS entropy
remain outside this control. This models the characteristic-value boundary, not
native controller scheduling or OS Bluetooth APIs. L2CAP is explicitly
unavailable; capability advertisement reports GATT support and the configured
frame limit. Wi-Fi, flash, physical reset, sleep, and
additional medium models remain future work. Frame/BLE time coordination is
available through the manual bridge below.

## Event-aware stepping

For registered actors on the private paused Tokio runtime,
`ManualTaskRunner::advance_to_next_wake(horizon)` now stops at the earliest
actor wake, medium event or caller horizon. It requires one-millisecond ticks,
settled actors and exclusive driving. Actors are not polled while time moves;
medium effects settle before actor polling resumes. This discovers Tokio timers,
not Embassy timers or external worker completions. Explicit-boundary stepping
remains available. See the [heap and deadline evidence](measurements/heap-and-deadlines.md)
for a 24-hour real-node lifecycle scenario and the opt-in 8/32/128-node heap probe.
The Embassy fixture additionally bounds each step by its observable timer queue;
see [Embassy parity evidence](measurements/embassy-parity.md) for coordinated
timer, complete-wire replay and process-isolated heap coverage.
The [mixed-runtime replay extension](measurements/mixed-runtime-replay.md) compares
whole bidirectional Embassy/Tokio traffic through a partition and reconnection,
using explicit entropy and the shared test-only BLE readiness selector.
The [Embassy restart extension](measurements/embassy-node-restart-replay.md)
reconstructs a receiver at its former address without resetting the surviving
sender or shared clock, and checks stale-handle and old-link isolation.
The [Embassy paired-fleet checkpoint](measurements/embassy-fleet-replay.md)
adds 8/16-node concurrent bidirectional replay and process-isolated allocation
measurements, with static fixture storage reported separately.
The [timer-capacity follow-up](measurements/embassy-timer-capacity.md) guards
host queue admission before upstream eviction, measures pending/peak waiters,
and adds fleet-wide response timeout and recovery replay.

Both frame and BLE media expose an atomic `MediumSchedule` snapshot containing
their current tick and earliest scheduled event, if any. `VirtualBleLab` exposes
the same view. Queued receptions and runnable runtime tasks are not represented
by this snapshot; an absent scheduled event does not mean the runtime is idle.

`advance_to_next_event(not_after)` stops at the next scheduled tick or the
caller's boundary, whichever comes first. It settles all events at that tick,
preserving delivery-sequence order for frames and radio-ID order for BLE ties.
Events due now settle without moving time. Between calls, runtime reactions may
change the next event; advancement recomputes it under the medium lock rather
than trusting an earlier snapshot. Existing bulk-advance methods are unchanged.

Existing queue and work limits still apply. A BLE same-tick batch exceeding its
emission budget fails before any clock, trace, schedule, or queue mutation.
Backward advances also fail without mutation. The numeric final tick is a real
deadline, not an idle sentinel; an exhausted periodic schedule does not wrap.
Frame lookup uses the ordered pending-delivery map. BLE lookup currently scans
attached radios without retaining another schedule index. This establishes
semantics, not a many-node scheduling throughput claim.

These medium APIs alone do not advance Tokio time or establish task quiescence.
The caller still owns coordination and the total scenario work budget.

```console
cargo test --locked -p prns-simulation --lib stepping
```

## Manual coordinated time

The opt-in `controlled-time` feature provides `ManualTimeDriver`, which owns one
paused, current-thread Tokio runtime for an explicitly polled scenario. It binds
either a frame medium, a BLE lab, or both to an explicit nonzero whole-millisecond tick
duration. Nonzero medium origins are supported. Merely polling a pending future
never advances time; snapshots validate all participating clocks instead of silently
resynchronizing them.

An advance stops at the next medium event or the caller's boundary. Medium
effects settle first, then Tokio time moves to that same instant, before the
caller next polls its futures. Emission-budget, backward-time, and clock-range
refusals leave all clocks unchanged. Timers must be constructed inside the
polled futures, and creation, use, and destruction of the driver are synchronous
operations outside any other Tokio runtime.

The boundary must account for the caller's known runtime deadlines: the driver
does not inspect Tokio's timer queue. Advancing past an earlier deadline would
still coalesce timers. It also does not poll timers to completion, infer
quiescence, order branches within a future, or automatically run a scenario.
Poll and step budgets remain explicit responsibilities of the scenario.

Live spawned async tasks are refused because their execution is not controlled
by this polling API. Blocking work, exported runtime handles, independently
advanced clocks, and concurrent medium mutation are unsupported. Detected clock
drift is a typed error; discard that driver. An error discovered after polling
does not undo the polled future's effects.

Tests cover a frame delivery coinciding with a production `TokioClock` deadline,
the production BLE supervisor's exact 10-second handshake timeout through
`Fleet::detached`, and emission-budget refusal followed by retry. They also cover
nonzero origins, duration validation, clock overflow, external drift, and spawned
task rejection. This is not a general-purpose full-node executor: nodes must use
paths that do not spawn background work. It does not change shipping runtime
behavior. The registered simulation suite enables
the feature; focused commands are:

```console
cargo test --locked -p prns-simulation --features controlled-time --lib manual_time
cargo test --locked -p prns-simulation --features controlled-time --test manual_time
```

`ManualMedium::FramesAndBle` coordinates one frame medium and one BLE lab with
matching origins. It selects their earliest event, settles BLE then frame effects,
and advances the runtime only after both complete. BLE emission-budget refusal
leaves all three timelines untouched. [Mixed-medium evidence](measurements/mixed-medium-clock.md)
includes a real transport bridging the two interfaces, exact request timeouts
during frame partition, continued BLE traffic and recovery on the original links.

The [mixed-bridge recovery follow-up](measurements/mixed-bridge-recovery.md)
distinguishes short BLE partition/radio disable from bridge process restart.
Radio-only interruptions preserve frame-local traffic and allow existing logical
links to resume after reconnection. Bridge restart loses forwarding state and
requires fresh links. Both CoreBluetooth/BlueZ profile assignments run repeated
cycles, with exact timeouts, membership, clock origins and cleanup assertions.

### Wake-driven scenario actors

`ManualTaskRunner` borrows a manual driver for the entire lifetime of a bounded
set of actor futures. Capacity is an explicit nonzero value. Each `poll_next`
call polls at most one ready actor and returns `Pending`, `Completed` with its
typed output and task ID, or `Idle`. The scenario must bound its total calls;
a self-waking actor cannot turn one call into an unbounded polling loop. Futures
must still cooperate by returning from each individual poll.

New actors start ready. Subsequent polls require a wake; repeated wakes coalesce
into one ready entry. Selection follows cyclic admission order across ready IDs,
so an immediately self-waking actor does not monopolize a fixed set. Ordered
indexes select ready actors without scanning dormant futures. Completion removes
both registration and readiness before dropping the future, returns the output
without retaining it, and frees capacity. IDs never repeat within a runner and
fail closed on exhaustion. Retained stale wakers cannot revive completed actors
or keep the scheduler alive. Dropping the runner drops its remaining futures.

Actor futures may be non-Send. They are polled in the driver's Tokio timer context
without entering Tokio's task scheduler, preserving cooperative-yield wakes and
leaving task order under the runner's control. No spawned tasks are supported.
The driver cannot be accessed independently while borrowed by the runner, and
its existing clock-drift and spawned-task checks still apply. Errors detected
after a poll do not undo actor effects or recover a completed output; discard
that runner and its driver.

`advance_to_next_event` refuses with `ReadyTasks` while registered actors are
ready. After settling ready work, the scenario still supplies its known runtime
deadlines as boundaries; the runner does not inspect Tokio's timer queue. `Idle`
means only that no registered actor was ready when checked, not that all runtime
or external work has settled. Concurrent external wakes are safe to record but
do not establish deterministic replay or atomicity with time advancement.

A 1,024-actor channel test verifies exact per-actor poll counts: waking one actor
does not repoll its 1,023 dormant neighbors. Other tests cover readiness
coalescing, round-robin selection, cross-thread wakes during polling, capacity
and ID exhaustion, teardown, cooperative yields, and exact timer boundaries.
The production BLE supervisor regression now also verifies discovery and its
10-second silent-handshake timeout through wake-driven polling. These are actor
and detached-supervisor tests, not a 1,024-node capacity or throughput result.
Shipping runtime behavior is unchanged.

```console
cargo test --locked -p prns-simulation --features controlled-time --lib manual_time::tasks
```

### Full-node coordinated ring

The `manual_fleet` capstone runs 128 production `PrnsNode`s concurrently on one
manual task runner and an explicitly connected ring. Every node owns a real
engine, manifold, interface driver, and request router. Each uses the existing
`CryptoPoolConfig::Inline` mode, no persistence, no transport forwarding, and
packet-sized echo responses. This keeps the exercised path inside the manually
polled actors without modifying production behavior or starting background crypto
workers. It does not cover pooled crypto, compression workers, large Resources,
or persistence workers.

All nodes boot in the private runtime's timer context. The first 128
transmissions are delayed one tick: no node hears an announcement before the
advance, and afterwards every node has heard exactly its two neighbors. All 128
nodes then establish links to their next neighbor and exchange exact echo
payloads. Removing one edge leaves the other 127 exchanges working while the
affected request remains pending until its explicit 50-millisecond timeout.
Restoring the edge permits a fresh request on the existing link. The test checks
every node's elapsed production clock against medium and runtime time, then
requests orderly shutdown and verifies all actors complete successfully, every
interface detaches exactly once, and no delayed frames or actors remain. The
bounded trace must be complete and free of receive-queue or delivery drops.

The scenario has explicit endpoint, neighbor, receive-queue, pending-delivery,
trace, actor, and per-settlement poll limits. Its timed section advances one
millisecond at a time, draining ready actors between steps. This is short-horizon
full-node coordination with known time resolution, not automatic discovery of
all production deadlines or arbitrary event-jump safety. The shared manual-fleet
fixture sets boot origins from a fixed logical epoch plus coordinated runtime
elapsed time, including restarts. OS entropy still varies; no byte-for-byte
replay claim is made. This establishes
128-node correctness for a direct-neighbor workload, not routed multi-hop
coverage, total per-node memory cost, throughput, or a maximum fleet size.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet
```

### Selective host-node restart

Selective actor teardown is also available through `ManualTaskRunner::cancel`:
it drops one actor in the paused runtime's context without polling graceful
shutdown, retires its wake ownership, and returns its capacity. IDs are not
reused. This runs Rust destructors; it is not a physical-power-loss model.
Dropping the entire runner also retires all wakes and destroys its actors in
their runtime context. [Review repairs](measurements/review-lifecycle-trace.md)
cover this teardown boundary and borrowed trace inspection for packet observers.

Two four-node [restart scenarios](measurements/selective-node-restart.md) exercise
that seam with real production nodes. A delayed frame cannot cross into a new
node instance sharing the old interface identity; a request queued at the
receiver's teardown times out exactly while unrelated nodes keep exchanging.
Replacement and surviving nodes retain coordinated elapsed clocks, with distinct
boot origins. Flash recovery, persistent identity provisioning and BLE controller
reset remain outside this host-node lifecycle slice.

The [BLE restart follow-up](measurements/ble-node-restart.md) rebuilds a Tokio
node and its real BLE supervisor at the same address and identity, three times
per CoreBluetooth/BlueZ profile case. Membership recovers without duplicates,
obsolete Reticulum links cannot answer new requests, and an unrelated pair
keeps its existing link. Separate backend tests cover stale queued sightings,
blocked control sends, queued frames and partial reassembly across replacement.
These are host-node/backend lifecycles, not native controller or firmware resets.

The [transport restart follow-up](measurements/transport-node-restart.md) uses
four leaves and two forwarding nodes on five isolated segments. Either transport
is rebuilt twice while the other five nodes stay live. Its route table starts
empty; explicit announcements restore the expected paths and fresh links work
in both directions. Unlike reconnecting the same transport connection, restarting
the forwarder does not restore obsolete link mappings: old-link requests time
out exactly while fresh links and the unaffected side's original link work.
This verifies host recovery after explicit rediscovery, not automatic application
retry, persisted routes, alternate-route selection or physical power loss.

The [in-flight follow-up](measurements/transport-inflight-restart.md) stops either
forwarder immediately after a real request or response is queued at its ingress,
before that node can consume it. Positive controls resume from the same boundary
and complete normally; teardown cases time out exactly while unrelated traffic
and subsequent fresh links work. A lost response means the server already
processed the request: timeout is not proof that an operation never happened.

### Routed multi-hop coordination

The same test target also runs a 20-node transport topology: 16 clients, two
servers, and two forwarding nodes. Each client has an isolated point-to-point
segment to the first transport, each server has one to the second, and a single
segment joins the transports. The 19 segments use 38 virtual interfaces with
one permitted neighbor each. They share the lab's frame clock, not a broadcast
domain; there is no direct client-to-server delivery path.

The test retains production forwarding and ingress policies and the existing
inline-crypto mode. Both server announcements must reach every client. The
production route inventory must report three hops and the correct ingress
interface for each client's chosen server. Sixteen concurrent links then carry
exact request/response payloads across both transports.

Cutting the transport connection leaves all 16 cross-segment requests pending
until their explicit 50-millisecond timeouts, while both servers can still form
links and exchange requests through their local transport. Restoring the
connection permits requests on all existing cross-segment links. Orderly
shutdown must complete every node, detach all 38 interfaces exactly once, and
leave no actors or delayed deliveries. The bounded medium trace must remain
complete and show no receive-queue or delivery drops.

Discovery uses a bounded 10-second simulated window with one-millisecond steps;
its exact completion time is not fixed because production rebroadcast jitter
still uses OS entropy. This scenario announces only two destinations and does
not stress the default ingress guard's held-announce burst release. It covers a
tree and restoration of the same transport connection, not alternate-route
selection, transport restarts, thousands of routed nodes, or performance limits.
No shipping policy, runtime, or wire behavior changes are needed for this test.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet routing
```

## Large-fleet design requirements

Both media require an explicit topology choice: fully connected, or sparse with
a nonzero neighbor limit. Sparse nodes start isolated; symmetric reachability
changes validate both endpoints and their budgets before applying. Discovery and
transmission visit only adjacent nodes. A 1,024-radio chain test checks every
observation against its expected neighbor, with bounded queues and trace storage.
This is medium-level evidence, not a full-node capacity result.

BLE connection admission, partitioning, radio shutdown, and backend teardown use
per-radio connection indexes rather than scanning every connection in the fleet.
Each connection is indexed at both endpoints; their admission budgets also bound
retired entries, which are reclaimed when that radio is next touched. Endpoint
destruction closes the shared lifecycle without reacquiring the network lock.
The explicit fleet-wide `active_connection_count` remains a linear inspection.
This trades additional bounded host-side index storage for local operations;
it changes neither firmware memory nor the production peer buffer.

A 1,024-pair regression holds 2,048 virtual backends live, replaces 128 links,
checks capacity refusal and closed endpoints, verifies exact data delivery across
every replaced and untouched pair, and retires all connections. These are backend
and GATT tests, not 2,048 full production nodes. A separate ignored timing probe
has [local before/after measurements](measurements/ble-connection-index.md).

A separate churn regression replaces 2,048 advertisers sequentially around one
scanner and checks the complete bounded history and eviction count after every
observation. Only two backends are live in this test; it establishes retention
behavior under churn, not concurrent full-node capacity or a throughput claim.

Radio identities are monotonically issued 64-bit values scoped to one medium.
They are never reused, so stale handles, queued observations, and trace entries
cannot alias a replacement. Issuance uses constant bookkeeping rather than a
history or recycling table. The final representable ID is issued once, then
attachment fails with `RadioIdsExhausted`; IDs never wrap. Required live-radio
capacity remains a separate limit, and rejected attachments consume no IDs.
IDs are wider host-side values, not a firmware or BLE wire-format change.

A backend regression reuses one address through 65,537 advertiser attachments,
requiring fresh discovery every time and exchanging data after the old 16-bit
ceiling. History and trace stay bounded. Detaching a radio wakes all pending
observation readers with `UnknownRadio`, even if its address is already reused;
the departed radio's topology edges are not inherited by its replacement.

Frame reachability is sampled at transmission; delayed frames already in flight
retain their original recipients. BLE lab partitions instead close queued and
established links before returning and prevent dialing previously seen peers
until reachability is restored. Previously queued sightings remain historical
observations, not permission to establish a connection.

Many-node scenarios are a first-class target, not a sequence of isolated
two-node tests. The coordinated ring establishes a 128-node correctness baseline;
the 20-node transport scenario adds routed multi-hop and partition recovery.
Thousands of production nodes remain a scale-test milestone, not demonstrated
capacity or a promised limit. BLE now also has a coordinated 16-node ring.

The [128-node churn slice](measurements/full-node-restart-waves.md) adds simultaneous
32-node restart waves across 64 isolated pairs, in both cohort orders. Unaffected
pairs keep their original links while lost and obsolete-link requests time out
exactly. A whole-fleet abort case drops all nodes plus 64 pending requests, then
rebuilds on the same clock. These are concurrent Tokio production nodes with
inline crypto over the frame medium, not routed/BLE fleet scaling or firmware
power-loss evidence. Capacity refusal, per-node clock origins, pair isolation
and complete attachment cleanup are checked explicitly.

- Run production nodes on a shared asynchronous runner, without requiring a
  hardware-emulator process per node or substituting simplified protocol nodes.
- Model explicit, sparse reachability for chains, clusters, bridges, and network
  partitions. Discovery and delivery should visit reachable neighbors rather
  than the whole fleet. Deliberately dense scenarios still incur the cost of
  their actual interactions; they are a separate stress case.
- Advance directly to due events with explicit work budgets, including delivery
  fanout. Bring production deadlines under controlled time before claiming
  deterministic full-fleet replay or accelerated long-duration scenarios.
- Bound queues, active links, discovery history, and diagnostics. Support repeated
  node churn without exhausting lifetime identifiers. Use aggregate counters
  alongside selective bounded traces.
- Measure memory per node and active peer, event throughput, and wall time per
  simulated interval. Scale runs must retain correctness assertions for delivery,
  recovery, backpressure, and cleanup, not merely demonstrate that nodes start.

Remaining obstacles include full-node deadline discovery, background-worker
coordination, models beyond the coordinated frame/BLE media, and measured total
per-node and per-peer allocation.
The transport-sized BLE receive buffer removes one known large allocation, not
all of those costs. Native queues, scheduler storage, and larger production-node
scales still need evidence.

A 16-node BLE ring exposed an interface-inventory lifetime race during
same-ID peer replacement. The Tokio runtime now retains the departing attachment's
own status registration, preserving replacement inventory and retiring only the
old traffic counters. Its [focused lifecycle regression](../../prns-runtime/impls/tokio/README.md#interface-replacement)
fails against the prior implementation. The ring additionally exposed a mutual
send stall under bounded GATT backpressure. The no-std BLE core now owns
`send_frame_duplex`, keeping one send alive while receiving and forwarding whole
frames; Tokio uses it with one additional bounded 564-byte outbound buffer.
Focused tests cover simultaneous fragmented sends, exact custody/completion,
receive failures, invalid lengths, and send completion during forwarding pressure.

The passing ring uses 16 production nodes, sparse two-neighbor reachability,
four-entry GATT queues, and 20-byte data values. All nodes discover and announce,
establish links, and concurrently exchange 256-byte echo requests. Disabling one
radio preserves unaffected exchanges; re-enabling it restores exact neighbor
inventory and traffic on all existing links. Shutdown leaves no actors or links,
and the complete bounded trace records each radio's detach exactly once.
Discovery has a 60-second simulated deadline; OS entropy still prevents exact replay.
Alternating Apple/BlueZ protocol endpoints exercise shared handshake decisions,
not native OS Bluetooth APIs, controllers, L2CAP, or Embassy firmware.

The separate `bluetooth-auto-embassy` PR suite runs actual Embassy BLE component
tests on the host, including checked receive fan-in and Trouble's bounded pools
and queues. Tokio, Embassy, and the duplex core share the same checked receive
boundary; a faulty backend cannot dispatch an out-of-buffer reported length.
Embassy fanout uses the shared receive driver until all selected sends settle,
including reception for unselected peers and early-finished senders. Forwarding
into its shared lane is serialized by an async mutex. Component tests exercise
concurrent receive/send progress, forwarding pressure, completion ordering,
failure isolation, and accounting through real Embassy fleet lanes. Unsettled
forwarding is tracked independently of send completion for safe cancellation.
Receive pumps remain scoped to fanout; independent receive tasks are still a
separate runtime change.

The `embassy_ble` integration target runs the real Embassy BLE supervisor against
the same virtual BLE backend used by Tokio. It verifies a silent peer stays
connected at 9,999 ms and is retired at 10,000 ms for both ESP32 and nRF52 protocol
endpoints, with exact recovery counters and no premature member admission. A
two-supervisor scenario checks native handshake admission, member registration,
and both ends' teardown after disabling one radio.

A three-node scenario runs complete production Embassy nodes, including command
settlement, routing, fleet lanes, and application delivery, on the manual clock.
One hub connects to two leaves through 20-byte GATT values and four-entry fragment
queues. Every node sends a 256-byte payload concurrently; the hub alternates
all-peer fanout and a selected peer while receiving both leaves' traffic. Whole
delivery values, command settlements, and per-peer RX/TX deltas must match, and
each exchange must settle without advancing time. Disabling the hub removes all
members and links; re-enabling it must restore the exact topology and successful
traffic within 60 seconds of virtual time. Dropping the actors closes all links.

Two mixed-runtime scenarios pair a complete Embassy node with a complete Tokio
node using ESP32/Apple and nRF52/BlueZ protocol endpoints. Both announce, open
encrypted links, and concurrently exchange exact 256-byte echo requests through
20-byte GATT values and four-entry fragment queues. Each operation must settle
without advancing virtual time. Tokio uses its existing inline crypto mode.

A forced BLE disconnect removes both member inventories. The 60-second
advertising interval allows the Reticulum links to expire before rediscovery;
both nodes must report exactly those link IDs with `Timeout`. Requests on the
expired links must promptly return `Rejected(NoSuchLink)`, including empty
requests and a Resource-sized Tokio payload. Fresh announcements and links must
restore encrypted request/response. Dropping the actors leaves no connections
and detaches each radio exactly once. This regression exposed the
[Tokio request-admission fix](../../prns-runtime/impls/tokio/README.md#request-admission):
the shared core now distinguishes link rejection from transport size selection.
The [local correctness evidence](measurements/mixed-runtime-ble.md) records the
red/green regression, verification commands, and coverage limits.

The same scenarios send a Resource-sized request beyond the Embassy echo's
admission limit. It must promptly return the exact typed peer rejection, then
allow an ordinary exchange on the same link. This exposed a separate shared-core
request settlement bug; [Resource settlement evidence](measurements/resource-request-settlement.md)
records its regression and receipt-ownership checks.

Its private clock bridge bounds automatic steps by Embassy's next timer deadline,
then mirrors successful medium/Tokio steps into its test time driver before
polling actors. The existing wake-driven runner retains
explicit actor and poll budgets; refused ready-actor and backward-time steps
leave all clocks unchanged. A process-wide lease serializes these scenarios and
resets the timer queue only after the actors are dropped. This test fixture
defaults to eight actors and uses a guarded 1,024-entry host timer queue; these are scenario bounds,
not an Embassy fleet scale claim. Submillisecond Embassy deadlines are refused
before any clock moves rather than rounded to the shared millisecond timeline.

The tests are included in `virtual-device-simulation`, run alongside Embassy
component tests in PR CI and by the relevant pre-push gate. The clock, backend,
and deterministic entropy source are confined to tests. The node fixture uses
host `GrowableHeap` storage and a fixed number of leaked allocations for the
runtime's static APIs; it establishes behavior, not embedded memory use or
large-fleet capacity. ESP32/nRF52 endpoint labels exercise shared protocol paths.
Native HCI/Trouble execution, board firmware, and RF still require separate
evidence. The mixed-runtime scenarios retain Tokio's production OS entropy;
they do not establish byte-for-byte deterministic replay.

```console
cargo test --locked -p prns-core interfaces::bluetooth_auto::duplex
cargo test --locked -p prns-simulation --test ble_peer_frames
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble::
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble
```

### Packet response limits

Two `embassy_ble` scenarios exercise exact packet response limits on the same
ESP32/Apple and nRF52/BlueZ pairings. Concurrent requests in both directions
accept 255 and 256 bytes, refuse 257 and 258 bytes with `ResponseTooLarge`, and
then succeed on the same links. Bin8/bin16/bin32 values also fill exactly the
256-byte completion capacity; their headers count because Rust callers receive
them unchanged. Shipping buffers, queues, and wire bytes are unchanged.

The scenarios exposed and now guard the shared-core two-byte undercount that
previously let Tokio accept oversized packet responses. Embassy's final buffer
already refused them; now the common ingress boundary does so consistently.
The [local evidence](measurements/packet-response-limits.md) records verification;
the [accounting plan](../../prns-core/plans/response-size-accounting.md) records
the remaining Resource admission work.

### Mixed-runtime Resource transfers

Two additional `embassy_ble` scenarios transfer exact 1,200-byte application
payloads with ESP32/Apple and nRF52/BlueZ endpoints. Tokio sends a Resource-backed
request to Embassy and receives its Resource echo. Both nodes concurrently ask
for Resource-sized replies on independent links. A desktop response limit one
byte below the encoded response value is refused with `ResponseTooLarge`, while
the exact value limit succeeds; the embedded responder sees `RejectedByPeer`.
Embassy also receives an exact 2 KiB response into its existing 2 KiB completion
buffer. Further replies and another upload succeed on those same links.

These scenarios use explicit 2 KiB request-routing and response-completion
capacities in the host fixture. Existing small fixtures and shipping capacities
are unchanged. The two-frame Embassy egress lane, four-entry GATT fragment queue,
and 20-byte values remain bounded; transfers may need protocol retries. The
private runner advances in one-millisecond steps only when actors are idle,
within a ten-second operation deadline and an explicit 256 polls per tick;
existing timeless completions retain their 128-poll bound. RTTs must equal
elapsed controlled time. Tests also cover deadline completion/refusal and prevent
a self-waking actor from spending future ticks' poll budgets without advancing.

Dense application bytes explicitly pass the production uncompressed preflight,
keeping background compression workers out of this scenario. Its 4,096-event
trace must remain complete, both radios must detach exactly once, and all BLE
connections must close. This is not compressed/segmented Resource, native OS
Bluetooth, firmware memory, hardware, or many-node scale evidence.

The [original evidence](measurements/resource-transfers.md) records the initial
envelope-limit observation. The [whole-Resource correction](measurements/whole-resource-response-limits.md)
lists current checks and limits; the [accounting follow-up](../../prns-core/plans/response-size-accounting.md)
tracks the remaining segmented response work.

The [whole-file correction](measurements/metadata-resource-response-limits.md) adds
two metadata-bearing scenarios through the shared static-file command on both
runtime lanes. Exact 1,200-byte file budgets succeed; one byte less fails after
verification without delivering content. Envelope-shaped file prefixes remain
literal, and Embassy receives a full 2 KiB file into its existing completion
capacity. Transport proofs acknowledge delivery even when the requester's final
value budget refuses the body. Further requests reuse the links. The host-only
fixture enables Embassy's existing `large-static-responses` feature; no shipping
capacity changes. These scenarios do not exercise Tokio's streaming-file
convenience API or its background compression workers.

### Segmented responses through both runtimes

Two further `embassy_ble` scenarios force a 1,200-byte static file through a
test-only 512-byte Resource transfer window, with one incoming transfer, one
outgoing transfer and one assembly slot per direction. Other tables retain the
existing host storage. Both ESP32/Apple and nRF52/BlueZ endpoint pairings exercise
the real Tokio and Embassy nodes, shared segmentation logic and virtual BLE link.

Raw request journals show exactly three ordered segments on the expected link,
one consistent request ID, the complete literal file and exactly one terminal
settlement whose RTT matches controlled elapsed time. Zero-budget offers fail
with `ResponseTooLarge` without a whole response or any segments. Subsequent
raw and buffered requests succeed on the same links, reusing the single transfer
slot. Both radios detach once and the bounded BLE trace remains complete.

The private raw-response observer retains at most eight events, each payload
bounded to 2 KiB, and wakes through notifications rather than polling in a loop.
Normal request futures still use each production runtime's completion path; the
observer does not replace delivery or reconstruct their returned values.

No production behavior, capacities or wire formats change. This establishes
successful segmentation, early refusal and reuse. See
[coverage and verification](measurements/segmented-resource-simulation.md).

Two interruption scenarios use an explicit two-radio topology. After exactly
the first verified response segment, an awaiting test actor isolates the radios
before returning control. Each requester must receive exactly one `Timeout`
settlement and no further segments or whole response. Restoring reachability
allows rediscovery; both old Reticulum links expire, and fresh links must pass
the complete refusal/raw/buffered sequence on the same node instances. Both
directions run for both endpoint pairings. The Embassy responder's pending
command must settle as `Respond(Resource(LinkClosed))` after link retirement.

The outage wait has a 20-second controlled-time budget, long enough for the
production inter-segment response grace; successful transfers retain their
10-second budget. Only these longer outage scenarios increase their bounded BLE
trace to 131,072 events, and still require zero discarded events. Discovery
retains its existing 60-second budget. Observer ordering and notification wakeups
have focused tests. See [interruption evidence](measurements/interrupted-segmented-resource-simulation.md).

Buffered-request interruption now uses a private, single-slot wire gate around
the test backend. It holds the second outgoing Resource advertisement before
GATT fragmentation; the test isolates the radios and releases the held send.
Paired raw-command cases calibrate that exact cut against one verified segment,
and ordinary Tokio/Embassy request futures must return `Timeout`, never partial
success. Both directions and endpoint pairings recover using the same nodes;
both Embassy request slots must also handle concurrent packet requests afterward.
A no-outage control releases the gate and checks the full file and RTT unchanged.
The gate retains only a header and bounded state, not payload copies or an event
queue, and cancellation releases its ownership. It is confined to this test
binary, with no shipping runtime or general simulator-library changes. See
[buffered interruption evidence](measurements/buffered-resource-interruption.md).

Those original interruption cases establish reuse after old-link retirement.
Subsequent scenarios also exercise same-link assembly reclamation, receipt
pressure, late continuations and exact cumulative response-value limits. The
[response accounting plan](../../prns-core/plans/response-size-accounting.md)
tracks the shared-core corrections and distinguishes mixed-runtime reproductions
from direct engine fixtures.

The [buffered competition matrix](measurements/buffered-response-competition.md)
tests both real requester APIs before the first verified segment, between
segments and during continuation reception. The
[retired-response matrix](measurements/retired-buffered-response.md) adds late
packets after success or timeout while a fresh request uses the same link.
The [abandoned-waiter follow-up](measurements/abandoned-request-waiters.md) checks
local future cancellation separately from protocol retirement, including reuse
of both bounded Embassy completion slots and the runtimes' different orphan
completion routing. All use bounded observers and ordinary responder APIs.

The [partial-response cancellation matrix](measurements/abandoned-segmented-responses.md)
drops callers after one or two verified file segments, then either resumes the
transfer or lets it time out. It checks exact orphan suffix/settlement routing,
concurrent waiter reuse and reclamation of the one assembly slot by another
link. Sender and receiver timeout observations are distinguished explicitly;
Tokio's private timeout cleanup is checked within a bounded virtual-time window.

Optional [seeded actor scheduling](measurements/seeded-actor-scheduling.md) adds
reproducible alternative cyclic actor orders, with 128-node restart coverage.
The default remains admission order; this does not seed production entropy or
provide complete packet replay.

The [overlapping bridge outage matrix](measurements/overlapping-bridge-outages.md)
uses those alternative actor orders to isolate frame and BLE paths together,
then restore them in either order. Local traffic resumes on the restored side
while end-to-end traffic remains blocked until both media recover. Concurrent
requests then reuse the original links without restarting any node.

The [mixed in-flight boundary matrix](measurements/mixed-inflight-outages.md)
cuts connectivity after a real request or response has reached the bridge's
frame queue, before the bridge polls it. It distinguishes losing the remaining
BLE path from cutting a frame hop the response has already completed, with an
uninterrupted control at every selected boundary.

The [partial-GATT request matrix](measurements/mixed-fragmented-outages.md)
uses bounded per-connection data counters to cut after two fragments are queued
or consumed, before a complete frame exists. Both request directions recover on
the original logical links. These counters retain no packet history and reset
with connection replacement; saturation is explicit.

The [partial-GATT response matrix](measurements/mixed-fragmented-responses.md)
adds the returning half of the exchange. Both request and response boundaries
now match the latest parsed packet header and logical link before inspecting
fragment deltas, avoiding confusion with protocol proofs. Only the latest header,
frame length and counter baseline are retained per direction, never payloads.

The [fragmented-response caller-cancellation matrix](measurements/fragmented-caller-cancellation.md)
keeps the BLE connection intact while dropping the awaiting caller at those
same boundaries. Two new requests reuse the logical link either before or after
the abandoned reply drains, with exact payload checks and a later check at the
old caller's response deadline.

The [request-side cancellation follow-up](measurements/fragmented-request-cancellation.md)
moves cancellation earlier, before the request has finished crossing BLE. The
after-drain cases verify that the request still finishes and the peer emits a
complete response: dropping a local waiter does not retract accepted work.

The [cancellation-plus-loss matrix](measurements/fragmented-cancellation-loss.md)
then combines caller abandonment with a BLE cut before those fragments drain.
A live timeout control distinguishes silent caller retirement from stalled
timer progress; recovery reuses the original logical link.

The [early-recovery matrix](measurements/fragmented-early-recovery.md) reconnects
before the old deadline, completes fresh concurrent traffic while an older
request remains pending, then checks that only the older live caller times out.

The [repeated-loss matrix](measurements/fragmented-staggered-timeouts.md) builds
three outstanding lost exchanges across rapid reconnects. Fresh traffic fills
the existing actor budget while each older caller retains its own deadline.

Two follow-ups check [retiring the middle caller](measurements/middle-caller-retirement.md)
without disturbing neighboring deadlines, and [alternating request/response losses](measurements/mixed-exchange-loss.md)
while callers from earlier outages remain pending. Both reuse the same bounded
fleet and exact deadline/completion assertions.

The [reverse-deadline cases](measurements/reverse-request-deadlines.md) require
later callers to retire before earlier ones. The [shared-deadline cases](measurements/shared-request-deadline.md)
require three staggered callers to settle as one exact batch. Both alternate
request/response loss and recover before any old caller expires.

The [shared-deadline cancellation cases](measurements/shared-deadline-cancellation.md)
then retire the middle caller either before expiry or at the deadline tick
before actor polling. The two survivors must still settle together, with no
completion from the cancelled caller.

The [deadline-edge retirement cases](measurements/deadline-edge-retirement.md)
cancel the earliest deadline, the latest deadline, or every pending caller.
They check silent cancelled boundaries and fresh traffic between deadlines,
including when no old callers remain to drive timer progress.

The [partial deadline batches](measurements/partial-deadline-batches.md) combine
two equal deadlines with a distinct third deadline. They check both batch
orders and cancellation of one paired caller while retaining the other batch.

Independent conflicting whole-Resource peers still have direct core-engine
fixtures, not mixed-runtime injections. Pooled worker scheduling, complete
byte-for-byte replay, thousands of production nodes and Wi-Fi/physical-reset/flash/sleep
models remain separate milestones. The response-assurance work does not imply
those broader simulator capabilities are complete.

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble segmented
```

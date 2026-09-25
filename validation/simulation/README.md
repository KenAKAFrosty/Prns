# Virtual device simulation

This non-shipping crate drives the production Personal Reticulum runtime through
host-native models of the hardware and media around it. It does not reimplement
Reticulum behavior.

The foundation provides a deterministic, bounded frame medium and a production
`Interface` adapter. Its capstone runs two real `PrnsNode`s through announce,
link establishment, and request/response without sockets or physical hardware.

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
fragments. Each peer's smaller value limit applies to the connection, and the
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
Tokio time, matching the manifold's monotonic clock. Tests can pause that clock
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

Medium ticks and runtime time are still advanced independently; these tests do
not define a tick duration or a combined event-ordering policy. Full runtime
replay is not yet deterministic, and wall-clock boot timestamps and OS entropy
remain outside this control. This models the characteristic-value boundary, not
native controller scheduling or OS Bluetooth APIs. L2CAP is explicitly
unavailable; capability advertisement reports GATT support and the configured
frame limit. Wi-Fi, flash, reset, sleep, and a unified time driver remain future
work.

## Event-aware stepping

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

This is the medium-side prerequisite for a unified time driver: it does not yet
advance Tokio time, choose ordering between a timer and a medium event at the
same instant, or establish task quiescence. The caller still owns those steps
and the total scenario work budget.

```console
cargo test --locked -p prns-simulation --lib stepping
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
two-node tests. Hundreds and then thousands of production nodes are scale-test
milestones, not demonstrated capacity or a promised limit. The current full-node
capstones establish two-node correctness only.

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

Remaining obstacles include coordinated medium/runtime time advancement and a
measured accounting of total per-node and per-peer allocation. The transport-sized
BLE receive buffer removes one known large allocation, not all of those costs. Native
queues, scheduler storage, and full production-node scale still need evidence.

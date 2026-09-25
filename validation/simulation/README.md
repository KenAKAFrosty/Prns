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

The logical clock owns media delivery and advertisement scheduling. Production
runtime deadlines still use real time, so full runtime replay is not yet
deterministic. This models the characteristic-value boundary, not native
controller scheduling or OS Bluetooth APIs. L2CAP is explicitly unavailable;
capability advertisement reports GATT support and the configured frame limit.
Wi-Fi, flash, reset, sleep, and unified runtime time remain future work.

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

Remaining obstacles include real-time runtime deadlines and lifetime radio IDs.
The production BLE peer receive buffer also uses the global maximum wire-frame
size (524,352 bytes), despite its smaller transport MTU. Audit transport-specific bounds and measure
against the existing runtime before changing allocation policy; do not conceal
that cost in the simulator.

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

The logical clock owns media delivery and advertisement scheduling. Production
runtime deadlines still use real time, so full runtime replay is not yet
deterministic. BLE control messages and frames cross the backend trait seam;
native controller behavior, GATT framing, and L2CAP upgrades are not modeled.
Wi-Fi, flash, reset, sleep, and unified runtime time remain future work.

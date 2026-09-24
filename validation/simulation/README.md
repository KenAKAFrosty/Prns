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

The logical clock currently owns media delivery, not the production runtime's
deadline clock. BLE, Wi-Fi, flash, reset, sleep, and unified runtime time belong
above this transport-neutral seam in later reviewable slices. The production
protocol engine remains the system under test throughout.

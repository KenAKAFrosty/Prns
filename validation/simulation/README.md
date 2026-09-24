# Virtual device simulation

This non-shipping crate drives the production Personal Reticulum runtime through
host-native models of the hardware and media around it. It does not reimplement
Reticulum behavior.

The first slice provides a deterministic, bounded frame medium and a production
`Interface` adapter. Its capstone runs two real `PrnsNode`s through announce,
link establishment, and request/response without sockets or physical hardware.

The medium intentionally makes its limits and faults explicit:

- endpoint, receive-queue, and trace capacities are required configuration;
- channel tags are nonempty, bounded, and unique within a medium;
- scheduled frame drops use stable transmission ordinals;
- every accepted transmission and delivery outcome enters a bounded trace;
- trace eviction is counted instead of silently pretending the trace is whole.

This slice does **not** yet claim deterministic virtual time or model BLE,
Wi-Fi, flash, reset, or sleep. Those belong above this transport-neutral seam in
later reviewable slices. The production protocol engine remains the system under
test throughout.

# Shared-hub BLE replay

## Question and workload

Does exact replay survive when concurrent connections share a BLE supervisor,
rather than belonging to independent pairs?

Three real Tokio nodes form a star. The hub permits two peers and each leaf
permits one. Entropy, logical time, interface arbitration and BLE supervisor
arbitration remain explicit, using the existing production construction seams.
The hub announces its destination to both leaves; both establish Reticulum
links and submit distinct 256-byte requests concurrently through 20-byte GATT
values. Request actors are admitted together before the runner settles.

One leaf is isolated. Its member disappears at both ends while the hub retains
the other leaf. That unaffected leaf completes another request using its
original Reticulum link. Recovery admits a replacement connection for the
isolated leaf, and both leaves complete another concurrent request batch.
Captured connection IDs prove only the affected BLE connection was replaced.
Every captured value must belong to one of the two star edges.

## Evidence and limits

Cyclic and seed-7 actor schedules each compare nine fresh runs. Comparisons
retain every control/data value, direction, connection ID, discovery event,
phase boundary and actual application response, in original order. Different
schedules need not match each other. Changed host entropy changes wire values
without changing responses; an equal-length changed payload changes both.

Each run rejects capture eviction, trace eviction and dropped observations,
then proves all actors, connections and radio registrations retire. Bounds are
five actors, three radios, two neighbors per radio, 8,192 captured values and
262,144 discovery events. These are fixture capacities, not measured minimums.

No production behavior changed, and no new uncontrolled input or production
defect was observed. Shared fixture construction now accepts a compile-time
peer capacity, and traffic helpers accept explicit destination mappings so
paired and hub scenarios reuse the same real-node operations.

This establishes this bounded shared-supervisor workload, not all nested Tokio
selectors, arbitrary interleavings, routed mesh, native Bluetooth, RF, firmware,
long-duration acceleration or thousands-of-node performance. Scale measurement
and broader replay inputs remain separate roadmap work.

## Verification

Host: macOS arm64. Host Cargo invocations use `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay::star --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay --quiet
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

All listed checks passed. The ten-test BLE replay group passed twelve separate
test-process runs. The registered simulation suite passed in 75.027 seconds,
including 103 manual-fleet tests; the existing opt-in memory probe stayed ignored.
Firmware, hardware, other OS targets, integration capstones and performance
benchmarks are outside this test-only slice.

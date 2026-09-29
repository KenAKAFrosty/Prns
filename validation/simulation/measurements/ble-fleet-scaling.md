# Real-node BLE fleet scaling smoke

## Workload and method

The existing paired-fleet replay workload now accepts a compile-time even node
count from 4 through 128. That ceiling keeps this fixture within its existing
one-byte address/identity/entropy inputs and manual-fleet poll budget; it is not
a production node limit. There is no new simulator or protocol implementation.

Each pair discovers, announces, establishes a Reticulum link and exchanges a
256-byte request over 20-byte GATT values. One pair disconnects while all other
pairs exchange another request batch on their existing links. The affected
pair reconnects, then all pairs exchange a final batch. Shutdown verifies all
actors, connections and radio registrations retire. Capture must not evict,
discovery must not lose observations, and each phase checks connection identity.

The ordinary test runs 32 real Tokio nodes four times and compares complete
wire/discovery transcripts and responses. The opt-in probe exercises 8, 32 and
128 nodes in sequence, each with one unmeasured warm-up/reference run followed
by five timed runs. Every timed result must exactly match its reference.
All use seeded actor order 7 and explicit entropy and internal arbitration.

Timing uses host `std::time::Instant`, not paused simulated time. The measured
interval includes construction, protocol work, assertions, capture snapshots
and shutdown. It excludes the final cross-run transcript comparison and
returned-transcript destruction. References stay alive during timed runs.
No timing threshold gates CI. This is an opt-in debug-profile smoke probe, not
a publishable release benchmark or comparison with an older implementation.

## Initial result

macOS arm64, default Cargo test profile, `CARGO_INCREMENTAL=0`. No other agent
build/test command was running during this recorded pass. Times are milliseconds.

| Nodes | Live pairs | Completed requests | Min | Median | Max | Captured values | Captured payload bytes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 4 | 11 | 19.593 | 19.632 | 19.794 | 734 | 14,771 |
| 32 | 16 | 47 | 82.303 | 83.955 | 84.978 | 3,014 | 60,431 |
| 128 | 64 | 191 | 445.077 | 447.082 | 448.693 | 12,134 | 243,071 |

The 4x increase from 8 to 32 nodes took about 4.28x as long; the next 4x
increase took about 5.33x. Per-node lifecycle time rose from roughly 2.45 ms
to 3.49 ms. This warrants phase-level measurement before attributing the
increase to the runtime, scheduler, topology or test assertions. No algorithmic
complexity claim follows from three small host measurements.

Captured payload bytes exclude vector/record overhead, crypto state, protocol
queues, intermediate snapshots, reference transcripts and all other runtime
allocations. They are not RSS, peak heap, or a per-node memory estimate. Capacity
bounds scale with node count: 1.5 actors per node, one peer per node, 2,048
captured values and 32,768 discovery events per node. These conservative bounds
are not measured minimums or evidence that thousands of nodes fit in memory.

Production impact: none. This slice adds scale evidence and reuses existing
real-node behavior. Sparse independent pairs do not stand in for a routed mesh,
a high-degree hub, native Bluetooth, firmware or RF. Shared-hub ownership has
separate bounded correctness coverage.

## Reproduction and verification

Host Cargo commands use `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay::fleet --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay::fleet::scale::measure_paired_ble_fleet_scaling -- --ignored --exact --nocapture
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

The registered simulation suite passed in 70.791 seconds, including 104
manual-fleet tests with the timing probe ignored. The 32-node replay test was
additionally run in six separate test processes.
The 128-node probe remains opt-in; the registered suite does not silently claim
that measurement. No firmware, hardware, other OS targets or integration
capstones were rerun for this test-only change.

# Embassy paired-fleet replay and heap baseline

This checkpoint scales real Embassy nodes, not just virtual radio backends.
Eight and sixteen nodes form isolated pairs, alternating ESP32 and nRF52 protocol
profiles. Every node announces, establishes an outgoing protocol link, sends a
256-byte request and responds to its peer's concurrent request. All operations in
each phase are polled together; this is not a sequence of independent two-node
test runs.

## Bounds and ownership

The Embassy runner adapter now accepts explicit actor capacity, settlement poll
budget and the shared runner's scheduling policy. Existing small scenarios retain
their eight-actor/128-poll defaults. This fleet reserves one actor per node plus
one operation actor; the per-tick settlement budget is 128 polls per node. Total
completion work remains bounded independently of elapsed simulation time.

Each node retains the existing bounded Embassy command, completion and BLE lane
configuration. The fixture uses growable core storage on the host; these are not
firmware memory profiles. The global upstream generic Embassy timer queue remains
64 entries; this slice does not claim arbitrary fleet size. The fleet constructor
is compile-time bounded to even sizes from four through sixteen nodes.

The explicit topology permits exactly one neighbor per radio. Discovery capture
allows 64 events per node and wire capture 256 values per node. Tests verify exact
peer identities and connected status, pair connection count, whole application
responses, zero capture eviction, no remaining connections after teardown, and
matching attached/detached radio inventories. Static allocations count exactly
eight objects per node; they remain retained until process exit by design.

The concurrent operation helper owns its futures, polls every unfinished future
and retains each result in input order without polling completed futures again.
It does not spawn hidden runtime work or introduce another executor.

## Replay evidence

Under cyclic scheduling and seeds 7 and 41, each eight-node configuration compares
four complete runs and a changed-payload control. Each sixteen-node configuration
compares two complete runs. Whole transcripts retain ordered wire values,
discovery events and responses from every node. A different actor order need not
have the same transcript; each fixed order must reproduce its own result.

These 21 bounded runs prove paired-fleet replay for this workload. They do not
exercise routed meshes, multi-peer contention, fleet recovery, source reseeding,
long-duration timer pressure, persistence or native radios. The prior restart and
mixed-runtime scenarios remain complementary evidence, not implied fleet cases.

## Isolated heap probe

With `heap-profile`, an ignored probe launches a fresh child test process for
each size. DHAT measures requested allocations during the complete fleet run;
the returned transcript is then dropped. Direct static fixture bytes are counted
separately. No preceding fleet's retained allocations contaminate the next size.

Observed on macOS arm64, default test profile, decimal bytes:

| Nodes | Static blocks | Direct static bytes | Peak heap bytes | Total allocated bytes | Allocation blocks | Live with transcript | Live after transcript drop |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 64 | 74,368 | 943,440 | 1,127,864 | 3,064 | 125,096 | 74,944 |
| 16 | 128 | 148,736 | 1,881,402 | 2,258,480 | 6,106 | 249,872 | 149,568 |

Remaining live bytes include retained static objects and host overhead; they are
not reported as a production leak. DHAT excludes stacks, allocator metadata and
RSS. This workload differs from the Tokio lifecycle probe, so these values are
not a runtime-efficiency comparison, a firmware RAM estimate or a thousand-node
capacity extrapolation. Reusable static storage and larger Embassy fleet timer
capacity remain follow-ups before broad scaling claims.

Production impact: none. Only simulation fixtures and tests changed; scheduling,
entropy, timer-queue and node behavior reuse existing implementations.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble replay::fleet --quiet
cargo test --locked -p prns-simulation --features controlled-time,heap-profile --test embassy_ble replay::fleet::heap::measure_embassy_fleet_heap -- --ignored --exact --nocapture
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
git diff --check
```

Physical boards, firmware builds, ISA execution and other operating systems are
not implied by this host-only verification.

The registered simulation suite passed in 73.176 seconds. A second process-isolated
heap run reproduced all table values; these are observed baselines, not allocation
goldens or comparative timing measurements.

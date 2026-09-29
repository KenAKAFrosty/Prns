# Embassy clock, replay and memory checkpoint

The previous heap/deadline checkpoint measured Tokio fleets. Embassy already
ran real mixed-runtime request, Resource, cancellation and recovery scenarios,
but those tests did not establish automatic timer discovery, complete-byte
replay or a comparable memory baseline for its adapter.

## Coordinated timer ownership

The Embassy integration executable now installs a private observable time driver
instead of the upstream mock driver. It retains Embassy's upstream 64-entry
generic timer queue and one-megahertz tick frequency. The adapter observes that
queue's next expiration; it does not duplicate protocol timer policy.

The Embassy deadline bounds the shared runner's Tokio/medium/horizon step.
After that step, Embassy advances by the same elapsed duration before any actor
is polled. A process-global lease serializes scenarios and resets the queue only
after their actors drop. Completion and mixed-runtime discovery helpers use this
path instead of stepping every millisecond. Explicit-boundary tests retain the
manual path. Both total and per-tick poll budgets remain bounded independently
of the requested duration, avoiding duration-based `usize` overflow on 32-bit
hosts for day-long scenarios.

Tests prove:

- Alternating Embassy and Tokio timers rearm across 24 simulated hours.
- Concurrent five-millisecond Tokio and ten-millisecond Embassy timers each
  observe both clocks at their own deadline.
- A real Embassy BLE supervisor expires a silent handshake at exactly ten
  seconds even with a day-long horizon.
- A one-microsecond Embassy deadline returns a typed refusal with the whole
  clock snapshot unchanged. The shared driver remains millisecond-resolution;
  this adapter does not silently round finer deadlines.

External workers, native I/O and hardware interrupts remain uncontrolled. The
test adapter is not a new firmware time driver or an Embassy executor model.

## Complete Embassy wire replay

Two real Embassy nodes discover each other, announce, establish a link, echo
256 bytes and shut down. Four fresh runs compare complete ordered BLE wire and
discovery snapshots plus the application response. Capture eviction and active
connections after shutdown must both be zero. Changing an equal-length payload
changes traffic; independently changing the sender's entropy seed changes traffic
while preserving the response. The existing shared-core entropy source is reused.

This proves the bounded Embassy-only scenario. Mixed Tokio/Embassy fixtures still
use ordinary Tokio entropy sources and do not yet claim whole-byte replay.
Embassy reconnect/restart replay and large fleets remain separate milestones.

The subsequent [mixed-runtime replay extension](mixed-runtime-replay.md) adds
complete bidirectional Embassy/Tokio traffic through partition/reconnect. The
limits above describe this initial Embassy-only checkpoint.

## Static fixture storage and isolated heap measurement

Production Embassy APIs require static references for channels, lanes and entropy
ownership. The fixture still satisfies those contracts with retained allocations;
there is no unsafe lifetime extension or deallocation. An accounting helper sums
the direct `size_of` of those objects. Resetting its counters does not free them,
and these totals exclude nested heap allocations.

The opt-in heap test launches its exact test case in a fresh child process, then
installs a DHAT profiling session around the two-node replay. This isolates it
from static objects retained by earlier tests. After actors and the returned
transcript drop, static allocations remain until process exit by design.

Observed on macOS arm64, default test profile, requested bytes (decimal):

| Measurement | Value |
| --- | --- |
| Nodes | 2 |
| Direct static fixture blocks | 16 |
| Direct static fixture bytes | 18,592 |
| Peak requested heap bytes | 487,358 |
| Total allocated bytes | 517,192 |
| Allocation blocks | 517 |
| Live bytes with returned transcript | 25,742 |
| Live bytes after transcript drop | 18,976 |

The residual includes static fixture storage and host overhead; it is not a
production leak claim. DHAT does not measure RSS, stacks or allocator metadata.
This workload is not the Tokio paired-fleet workload, so these values are not a
per-runtime efficiency comparison or an MCU RAM estimate. Larger Embassy fleets,
repeated lifecycle memory behavior and safe reusable static storage remain open.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

```console
cargo test --locked -p prns-simulation --features controlled-time,heap-profile --test embassy_ble --quiet
cargo test --locked -p prns-simulation --features controlled-time,heap-profile --test embassy_ble heap::measure_embassy_fixture_heap -- --ignored --exact --nocapture
cargo clippy --locked -p prns-simulation --all-targets --features controlled-time,heap-profile -- -D warnings
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
./tools/prns verify
python3 validation/run.py verify
cargo test --locked --manifest-path docs/website/Cargo.toml --quiet
./tools/prns repo notices check-inputs
git diff --check
```

The registered suite passed in 120.261 seconds; the focused Embassy target passed
89 tests. The ignored heap probe ran separately and reproduced the table in a
second isolated run. Hardware, firmware, other operating systems, native RF and emulator
execution are not implied by this host-only evidence.

Production impact: none. This slice changes simulator fixtures, test-only driver
selection and development dependencies; shipping Embassy APIs and behavior are
unchanged. Existing shared-core fixes already reach both runtimes.

# BLE fleet phase costs and clock-validation work

## Question and change

The previous 128-node probe cost more per node than its smaller counterparts.
Phase checkpoints now divide that same workload into boot, discovery, initial
traffic, isolated traffic, recovery traffic and shutdown. A regression test
checks the exact checkpoint sequence and compares the observed run's complete
transcript with the ordinary no-observer run. Ordinary runs do not read the host
clock at these checkpoints. Timings remain outside replay values.

Inspection found that `ManualTaskRunner::poll_next` validates the driver before
and after each actor poll. Validation called `ManualMedium::schedule`, causing
BLE's next-event calculation to scan every registered radio, even though
validation only consumed the current tick. This repeated fleet-wide work during
traffic that did not advance simulated time.

The driver now checks current ticks through `ManualMedium::now`; only actual
advancement computes the next event. Mixed media still reject disagreeing
ticks. Runtime task checks, Tokio clock checks and checks before/after polling
remain in place. The mixed-drift regression now verifies that snapshots,
polling and advancement all refuse external drift, without polling the supplied
future or changing either medium's trace.

This is simulator-owned orchestration, not shared protocol policy. It belongs
in the manual-time adapter; moving it to no-std core would put host-executor
concerns in the wrong layer. Shipping behavior is unchanged.

## Comparative smoke result

macOS arm64, default Cargo test profile, `CARGO_INCREMENTAL=0`. Both versions
use the same phase-instrumented workload, one warm-up and five measured samples
per size. The baseline uses the exact pre-change driver implementation from
`7f6e63d8b`; the candidate changes clock validation as described above. No other
agent build/test command ran during either measurement. These are debug smoke
observations, not statistical confidence bounds or release benchmarks.

| Nodes | Prior median ms | Candidate median ms | Change |
| --- | --- | --- | --- |
| 8 | 20.033 | 20.775 | +3.7% |
| 32 | 85.470 | 80.363 | -6.0% |
| 128 | 451.697 | 319.182 | -29.3% |

The small-size increase is retained here; these samples do not establish a
speedup at every size. At 128 nodes the prior range was 451.114–455.434 ms,
versus 316.995–320.846 ms for the candidate.

| Phase, 128 nodes | Prior median ms | Candidate median ms | Included work |
| --- | --- | --- | --- |
| Boot | 52.544 | 52.695 | Lab/runtime construction, node admission and destination construction |
| Discovery | 73.767 | 61.316 | Reachability, BLE convergence, announcements and receipt |
| InitialTraffic | 179.832 | 132.122 | Concurrent link establishment, first requests, capture and identity checks |
| IsolatedTraffic | 68.386 | 32.191 | Pair isolation, inventory checks, unaffected requests and capture |
| RecoveryTraffic | 70.541 | 34.066 | Reconnection, replacement link, final requests and capture checks |
| Shutdown | 6.611 | 6.164 | Actor shutdown and cleanup/trace assertions |

Per-phase medians need not sum to the total median. Total timing also includes
local destruction after the final checkpoint. Phase intervals include the
preceding observer's small bookkeeping cost; phase timing does not separate
protocol computation from test assertions or capture allocation. The total
timer excludes returned-transcript destruction and cross-run comparison.

All 128-node runs still completed 191 requests, captured 12,134 wire values
(243,071 payload bytes), retained 1,358 discovery events, repeated exact
transcripts and verified complete cleanup. Those payload bytes remain a trace
quantity, not heap or RSS. This resolves a demonstrated avoidable simulator
cost; it does not prove thousands-of-node capacity or accelerate arbitrary
runtime deadlines.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time manual_time --quiet
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

All listed checks passed. The 41 focused manual-time tests passed; the registered
simulation suite passed in 69.554 seconds, including 105 manual-fleet tests.
The opt-in probe ran explicitly;
ordinary suite execution keeps timing measurements ignored. Firmware, hardware,
other OS targets and integration capstones are outside this simulator-only slice.

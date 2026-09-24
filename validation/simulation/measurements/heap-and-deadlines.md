# Heap baseline and deadline-driven simulation

## Heap baseline

The optional `heap-profile` feature enables the repository's existing DHAT
allocator tooling in the manual-fleet test executable. Default builds and
shipping crates do not install it. Run the exact ignored probe with one test
thread; DHAT measures the entire process, not an individual actor. Each fleet
size gets a fresh profiler. No reference transcript is retained during the run.
Phase snapshots separate boot, connected traffic, recovery and teardown.

The later [Embassy checkpoint](embassy-parity.md) adds a separate process-isolated
probe and observable timer adapter; its static-lifetime accounting must not be
inferred from the Tokio fleet results below.

The original paired fixture reserved 32,768 discovery events per node despite
retaining fewer than 13 per node in the measured runs. It now permits 64 events
and 256 wire values per node. Every run still asserts no eviction, no dropped
observations, exact responses, connection identity and complete actor cleanup.
These bounds belong to this short fixture, not a universal long-duration policy.

macOS arm64, default test profile, requested allocation bytes (decimal):

| Nodes | Prior peak | Bounded-trace peak | Total allocated bytes | Allocation blocks | Retained after returned transcript drops |
| --- | --- | --- | --- | --- | --- |
| 8 | 20,332,509 | 3,588,205 | 4,256,289 | 7,391 | 392 |
| 32 | 81,318,133 | 14,340,485 | 16,269,717 | 29,329 | 336 |
| 128 | 325,253,781 | 57,341,797 | 64,478,061 | 117,809 | 336 |

The 128-node reduction is about 82%. The remaining measured peak is about
448 KB per node in this particular paired workload. That includes simulated
media, actors, runtime state, protocol queues and capture snapshots: it is not
an embedded RAM estimate or isolated node cost. Boot live bytes at 128 nodes
were 37,593,592; after discovery they were 53,821,088. The final transcript
retained 967,887 measured live bytes before its drop. Residual bytes include
the still-live phase-sample vector and runtime/thread initialization; they are
reported rather than called a zero-allocation or zero-leak proof.

DHAT counts requested heap blocks/bytes, not allocator metadata, stacks, mapped
code or RSS; profiler-internal storage is excluded by the tool. The opt-in run
is a capacity baseline, not a timing benchmark. Allocation totals include
reallocation and temporary snapshots. No thousand-node capacity extrapolation
is claimed. The dependency and lockfile changes are confined to optional
instrumentation of the non-shipping simulation package.

A second isolated run reproduced the total allocation bytes/block counts and
post-transcript residuals, with phase peaks differing by a few kilobytes
(128-node peak: 57,343,589 bytes). These are observations, not exact allocation
goldens; complete wire replay is verified separately.

## Deadline coordination

`ManualTaskRunner::advance_to_next_wake(horizon)` uses the private paused Tokio
runtime's timer queue, rather than duplicating protocol deadline state. It
refuses ready actors, coarse ticks and backwards horizons. The earliest medium
event bounds the runtime wait. The wait stops when a registered actor wakes or
the bound expires; no actor is polled inside that wait. The driver then settles
medium effects at the observed tick before the caller resumes actor polling.
Timer/medium ties therefore expose settled media to actors.

The BLE emission budget is preflighted through that bound before runtime time
can move. This is conservative: an invalid upcoming emission batch refuses the
step even if an earlier timer could have changed the radio state. Callers can
still use the existing explicit-boundary stepping API. The preflight is not a
reservation against concurrent mutations; existing exclusive-driving rules
remain mandatory. Post-advance errors require discarding the runner.

An idle actor without a timer cannot block the operation indefinitely because
the caller must supply a finite horizon. Due/deferred wakes may return without
moving time; callers must settle actors and maintain their work budget. The
method requires one-millisecond ticks to preserve Tokio timer resolution. It
does not enumerate Embassy's timer queue or control external workers, native
I/O, spawned tasks, RF, or concurrent medium mutation. Existing explicit
stepping remains available for those manually coordinated fixtures.

## Evidence

- A timer actor sleeps/rearms hourly for 24 hours; exactly 24 advances observe
  each hour and the actor remains unpolled until explicitly resumed.
- Canceled timers do not cause dead actors to execute; an empty runner reaches
  its finite horizon.
- Concurrent timers preserve all same-deadline wakes; nonzero medium origins
  and an earlier caller horizon preserve the original timer deadline.
- A two-millisecond timer precedes a five-millisecond frame delivery; a rearmed
  timer tied with that delivery observes the settled medium.
- Ready actors, backwards horizons, unsupported tick widths and BLE emission
  overflow are refused. Overflow leaves both medium traces and clock snapshot
  unchanged.
- Two real nodes idle through one simulated day in one jump, then establish a
  link and complete a 256-byte request. After isolation, a real 50 ms request
  timeout stops automatic advancement at that deadline despite a horizon a
  further day away. Both nodes shut down cleanly; traces do not evict.

The long-duration node test is a lifecycle/timeout correctness test, not a new
byte-replay claim: it uses the ordinary node entropy sources. Existing explicit
input BLE replay tests remain separate and continue to exercise complete traces.

Production impact: none. Heap instrumentation, bounds and wake coordination
belong to the simulator; no protocol or firmware control policy is duplicated.

## Commands

All host Cargo invocations use `CARGO_INCREMENTAL=0`.

```console
cargo test --locked -p prns-simulation --features controlled-time,heap-profile --test manual_fleet ble_replay::fleet::heap::measure_fleet_heap -- --ignored --exact --nocapture --test-threads=1
cargo test --locked -p prns-simulation --features controlled-time --lib manual_time --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet long_duration -- --nocapture
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet ble_replay --quiet
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

Hardware, firmware, other operating systems and integration capstones are not
implied by this simulator-only verification.

The registered simulator suite passed in 87.517 seconds, including 106
manual-fleet tests. Its ordinary run leaves the profiling probes ignored;
the heap probe above was run separately and explicitly. The later added
origin/horizon and concurrent-timer regressions were also verified with the
focused 48-test manual-time group and feature-enabled Clippy.

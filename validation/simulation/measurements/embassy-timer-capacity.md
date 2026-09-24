# Embassy timer capacity and fleet deadlines

Inspection of pinned `embassy-time-queue-utils` 0.3.2 showed that its generic
queue handles overflow by waking and removing an existing waiter before admitting
the new one. Early wakes are legal, but in an overloaded deterministic fleet they
can create repeated ready work before simulated time advances. Node count alone
does not describe that pressure: queue entries belong to distinct wakers.

## Host-only admission guard

The simulation driver now uses upstream `ConstGenericQueue<1024>` through a thin
guard. A registry maps each distinct original waker to one tracked forwarding
waker; repeated registrations use `will_wake` to preserve coalescing. Upstream
still owns all expiration times, earliest-deadline updates and wake ordering.
The registry stores no duplicate deadline policy.

Before registering a new distinct waiter at capacity, the guard returns typed
`CapacityExceeded`, leaving the queue and existing waiters untouched. Embassy's
driver trait has no fallible scheduling return, so its integration adapter fails
the test explicitly on that result rather than permitting eviction. Expiration
forwards each wake and releases the registry entry. Pending, peak and capacity
counts are available to fixtures and retained in fleet transcripts.

The registry is bounded and does not retain expired registrations. Cancellation
does not remove a timer from the upstream queue: its registration can remain until
expiry or scenario reset, and still counts against capacity. A clock lease resets
the complete driver after actors drop. This is not unbounded fleet support or a
per-device hardware timer model. Queue searches and registry scans remain linear.

Tests cover whole-value capacity refusal with zero early wakes, admission after
expiry, repeated-waker coalescing, earlier/later deadline updates, rearming, and
release of tracked waker ownership. A real-driver test arms 128 distinct actors,
observes exactly 128 pending slots, steps with a day-long horizon to 50 ms, then
checks all 128 completed at that tick and pending occupancy returned to zero.

## Real fleet deadline workload

The existing 8/16-node paired fixture now has an additional workload. Each node
sends a request while its peer's response is deliberately dropped by the existing
wire gate. Every request must return `Timeout` at exactly 50 simulated milliseconds,
each gate must report exactly one lost response, and fresh concurrent requests
must then succeed on all existing links. Connections and radio teardown remain
checked. Cyclic and seeded actor schedules each repeat complete wire/discovery,
response, expiration and timer-occupancy transcripts at both sizes.

The operation future only drives real request APIs; it does not synthesize protocol
completion or manipulate request queues. Timer tests with 128 actors are not
evidence for 128 complete Embassy nodes: the real fleet coverage remains 8/16.

## Cost and scope

Repeating the same process-isolated echo heap probe on macOS arm64 gives:

| Nodes | Previous peak heap | Guarded peak heap | Added peak heap | Total allocation bytes | Allocation blocks |
| --- | --- | --- | --- | --- | --- |
| 8 | 943,440 | 943,824 | 384 | 1,128,280 | 3,074 |
| 16 | 1,881,402 | 1,882,170 | 768 | 2,259,344 | 6,125 |

Post-transcript live heap is unchanged at 74,944 and 149,568 bytes. Those retained
bytes include static fixture objects; they are not a production leak claim. The
larger fixed upstream queue is host process static storage, outside DHAT requested
heap accounting and outside the per-node static fixture totals. These are allocation
observations, not RSS, firmware RAM or performance benchmark results.

Production impact: none. The driver, registry and workload live only in the
simulation integration executable. Shipping timer queues, wake behavior and
firmware memory profiles are unchanged. No new dependency or timer implementation
was introduced; the previously pinned upstream queue remains authoritative.

## Verification

Cargo commands use `CARGO_INCREMENTAL=0` on macOS arm64:

```console
cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble clock:: --quiet
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

Hardware, firmware builds, other operating systems and ISA execution are not
implied. Larger real fleets and repeated lifecycle storage remain next steps.

The registered simulation suite passed in 60.295 seconds; the final focused
Embassy target passes 98 tests. Heap figures above come from the explicit ignored
probe, not from the ordinary test run.

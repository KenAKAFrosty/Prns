# Partial-GATT request interruption

macOS arm64, based on `881f3f82b`, 2026-09-27.

## Instrumentation

The BLE lab now exposes `data_snapshots()`: one snapshot per active connection,
including connections still handshaking. Each direction counts length-valid
send attempts, completed sends, fragments queued, characteristic values consumed
and complete frames reassembled. Control/handshake values are excluded.
Malformed consumed values still count; frames reassembled into an undersized
caller buffer count as reassembled even though delivery to the caller fails.

Counts saturate rather than wrap and set an explicit `saturated` flag. Tests
requiring exact deltas reject that flag. Cancelled sends remain started but not
completed; the difference is deliberately not described as an active-send count.
Each connection owns fixed counter storage behind a mutex. There is no retained
packet history, payload copy, event queue or lifetime counter aggregation.
Closed connections disappear from snapshots and replacements start at zero;
old endpoints cannot modify a replacement's counters.

This costs two five-counter-and-flag tuples plus a mutex per simulated connection,
and short accounting locks during virtual GATT send/receive. Reading snapshots
allocates a vector bounded by active connections and scans the existing index.
No zero-cost or performance-improvement claim is made. Concurrent native-thread
send/receive can be sampled between accounting events; these counters are not
queue-occupancy measurements or one atomic fleet-wide snapshot. The controlled
runner observes them between explicit actor polls.

## Full-node matrix

Two tests exercise cuts after queueing and after consuming a partial frame.
Each runs both request directions through the mixed frame/BLE bridge, both
CoreBluetooth/BlueZ profile assignments and four actor schedules (cyclic and
version-one seeds 0, 7 and u64::MAX). That is 32 combinations, each repeated
twice. Profiles use virtual GATT, not native OS Bluetooth stacks.

After discovery/link setup has settled, only the selected request is introduced;
no clock advancement occurs before the boundary. Counter deltas require exactly
one new send, exactly two queued fragments and no new completed send or
reassembled frame. Queue capacity is two and GATT values are 20 bytes.
The queued boundary requires zero newly consumed values and the last-polled
actor to be the sender. The consumed boundary requires exactly two newly
consumed values and the last-polled actor to be the receiver. Saturation,
unexpected completion, idleness, unrelated extra sends and budget exhaustion
fail rather than weakening the assertion.

An uninterrupted control first pauses at the same boundary, then verifies its
whole 256-byte response. The interrupted request is cut at that boundary by
isolating BLE. Its exact 50-millisecond timeout is verified while a frame-local
link continues exchanging data. No partial frame is exposed as a successful
request response. Restoring BLE recovers the original logical link and fresh
256-byte responses without replacing node actors. Route/ingress checks, actor
counts, per-node elapsed time and both medium/runtime clocks remain consistent.
Final shutdown checks complete detach and untruncated, non-overflowed traces.

Unit tests separately check counter values at partial-send and partial-receive
boundaries, cancellation, output-buffer refusal, both directional accounting,
saturation and old/replacement connection isolation.

## Production impact and limits

Only host simulator instrumentation and tests change. Production fragmenters,
reassemblers, supervisors, routing and runtimes remain unchanged. This is a
request-fragment boundary, not a response-fragment cut, real airtime model,
native Bluetooth run or hardware-reset test. Seeds select actor order, not
cryptographic entropy or byte-for-byte replay.

## Verification

Focused counter and GATT tests, the full-node partial-fragment matrix, root and
workspace tests, and simulation all-target clippy with warnings denied passed.
The registered simulation suite passed in 25.14 seconds. Repository
formatting/docs checks and `git diff --check` also passed.
Host Cargo runs used `CARGO_INCREMENTAL=0`; existing ignored tests remain ignored.

```console
cargo test --locked -p prns-simulation --features controlled-time --lib ble::gatt --quiet
cargo test --locked -p prns-simulation --features controlled-time --lib --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet mixed_fragmented --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

No firmware build, hardware smoke test, native Bluetooth, Miri/ISA or performance
benchmark result is claimed by this slice.

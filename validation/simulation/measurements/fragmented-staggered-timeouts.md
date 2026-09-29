# Independent deadlines across repeated BLE loss

macOS arm64, based on `853690dcf`, 2026-09-27.

## Scope

Four tests cover request/response and queued/consumed fragment boundaries.
Each runs both request directions, both CoreBluetooth/BlueZ profile assignments,
and cyclic ordering plus version-one seeds 0, 7 and u64::MAX: 64 configurations.
Each configuration runs two batches of three outage/recovery waves, using the
same logical crossing link and node actors throughout.

Each wave introduces a distinct 256-byte request, pauses at the exact
header-aware partial-frame boundary, cuts BLE and restores it while all older
callers are still pending. No caller is cancelled in this matrix.

An initial fixture assumption that reconnection necessarily advances time was
false: cached discovery can reconnect within the same tick. The final tests
therefore advance an explicit seven-millisecond virtual gap between waves,
settling intervening medium events with a bounded loop. They require strictly
increasing request start times rather than relying on incidental discovery
timing. This was a fixture correction, not a production bug.

## Assertions

- Every cut observes the selected request/response context, exact link,
  partial counters and last-polled actor before any whole-frame completion.
- Disconnection settles without moving time, clears the BLE connection/data
  snapshots and member inventories, and leaves frame-local traffic working.
- Every reconnection completes strictly before the first outstanding deadline.
  Route/ingress checks and two concurrent fresh responses succeed after each
  wave, with exact distinct payloads and unchanged pending-caller counts.
- At the third wave, three nodes, three older callers and two fresh callers
  occupy the existing eight-actor budget. No capacity is increased.
- Each older request times out at its own start plus 50 milliseconds. The
  completion map must contain only that caller; neither early settlement nor
  coalescing deadlines can pass. Pending actor counts decrease one at a time.
- Fresh concurrent traffic and frame-local traffic succeed between each older
  timeout. The second batch repeats on the same link with different payloads.
- Node IDs, clocks and final complete detach remain checked, with no pending
  deliveries/connections or truncated/overflowed evidence at teardown.

## Production impact and limits

Test-only addition using existing observers and replacement assertions.
No shipping behavior, simulator-library storage or timeout policy changes.
These are production Tokio nodes over virtual GATT profiles, not native OS
Bluetooth stacks. No private completion-table retention bound, raw application
journal audit, hardware/firmware result, Miri/ISA, performance benchmark or
exhaustive interleaving proof is claimed.

## Verification

Passed the focused four-test matrix, simulation all-target clippy with warnings
denied, root/workspace tests and the registered simulation suite (41.175 seconds).
Repository formatting/docs checks and `git diff --check` also passed.
Host Cargo runs used `CARGO_INCREMENTAL=0`; existing ignored tests remain ignored.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet staggered_timeouts --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

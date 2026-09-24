# Caller abandonment plus fragmented BLE loss

macOS arm64, based on `e05138855`, 2026-09-27.

## Scope

This follow-up combines the previously separate cancellation and transport-loss
cases. A real request or response is paused with exactly two BLE fragments
queued or consumed, the awaiting caller is dropped, and BLE is isolated before
any further actor poll can finish the exchange.

Four tests cover request/response and queued/consumed boundaries. Each runs both
request directions through the mixed frame/BLE bridge, both CoreBluetooth/BlueZ
profile assignments and cyclic plus version-one seeded schedules (0, 7 and
u64::MAX). There are 64 configurations, each repeated twice. The profiles run
over virtual GATT, not native OS Bluetooth stacks.

## Evidence

The existing observer checks the exact packet context and logical link,
new-send baseline, two queued fragments, zero/two consumed values, no whole
frame completion and the expected last-polled actor without time movement.

Cancelling the caller returns `Cancelled` and leaves both the data snapshot and
clock untouched. Isolation then settles to three node actors, no active BLE
connections, no active connection observations and empty member inventories.
The independent frame-local link still exchanges data.

A new, live request on the broken crossing link is started without advancing
time. At the exact 50-millisecond boundary, only that live caller may complete,
with its expected timeout. The abandoned actor must remain `NotLive`. This
positive timeout control prevents broken timer progress from masquerading as
successful silent retirement.

After restoring BLE, route/ingress checks and two concurrent, distinct 256-byte
responses succeed on the original logical link. A further deadline interval is
advanced with no registered caller completion, followed by another concurrent
request round. Node actor IDs and coordinated clocks remain correct. Final
shutdown verifies complete detach, no pending frame deliveries/connections and
untruncated traces without receive/discovery queue overflow.

The original actor capacity remains eight: three node actors, one abandoned
caller before cancellation, and up to two replacement callers after recovery.
No limits are enlarged to make recovery pass.

## Production impact and limits

Test-only change, using existing fragment observations and sharing the previous
same-link replacement assertions. No shipping code, allocation policy or
cancellation behavior changes.

The scenario observes registered actor completions, not raw application
journals or private completion-table size. It does not prove bounded private
retention for indefinitely unresponsive peers, exactly-once side effects,
native Bluetooth behavior, hardware reset recovery or exhaustive schedules.
No firmware build, Miri/ISA or performance benchmark is claimed.

## Verification

Passed the focused four-test matrix, root/workspace tests, simulation all-target
clippy with warnings denied, and the registered simulation suite (36.629 seconds).
Repository formatting/docs checks and `git diff --check` also passed.
Host Cargo runs used `CARGO_INCREMENTAL=0`; existing ignored tests remain ignored.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet cancelled_caller_survives --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

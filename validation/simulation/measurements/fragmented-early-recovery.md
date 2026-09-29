# Reconnection before an abandoned exchange's deadline

macOS arm64, based on `d1a65eabc`, 2026-09-27.

## Scope

The previous cancellation-plus-loss matrix restored BLE after the old timeout.
This follow-up deliberately reconnects before that deadline so fresh successful
traffic overlaps older pending state on the original logical link.

Four new tests cover request/response and queued/consumed fragment boundaries.
Each runs both request directions, both CoreBluetooth/BlueZ profile assignments,
and cyclic ordering plus version-one seeds 0, 7 and u64::MAX: 64 new
configurations, each repeated twice. The previous after-timeout cases remain.

## Assertions

The exact header-aware partial-frame boundary, caller cancellation, BLE loss,
empty member/connection inventories and uninterrupted frame-local traffic checks
are reused. A live request sent during the outage remains as a positive timeout
control alongside the retired caller.

For early recovery, that live request is not awaited before reconnecting:

- BLE reconnects and discovery settles strictly before the original
  50-millisecond deadline. A late reconnection fails the test.
- Three nodes and the older live caller remain registered. Two new requests
  then run concurrently on the original link, bringing the active actor count
  within the unchanged eight-slot budget.
- The exact replacement response map contains only the two fresh payloads.
  Both complete before the old deadline, leaving the older caller pending.
- Advancing to the original deadline yields precisely that older caller's
  expected timeout. No completion comes from the abandoned actor, and fresh
  success does not settle or retime the older request.
- A further deadline interval produces no registered caller completion, and
  another concurrent pair succeeds. Node IDs, routes/ingress, clocks and final
  detach checks remain intact.

The common replacement helper now preserves the actor count it found on entry
instead of assuming only the three node actors exist. Whole completion-map
assertions remain unchanged, so an unexpected older completion cannot be hidden
by this generalization.

## Production impact and limits

Test-only extension; no shipping behavior, simulator-library storage or timeout
policy changes. The result shows that reconnection and fresh traffic do not
resurrect the lost older exchange or interfere with its live caller's deadline
in these scenarios.

These are real Tokio nodes over virtual GATT profiles, not native OS Bluetooth
stacks. This is not a private completion-table retention bound, raw application
journal audit, exactly-once guarantee or exhaustive interleaving proof. No
firmware, hardware, Miri/ISA or benchmark result is claimed.

## Verification

Passed the focused four-test early-recovery matrix, simulation all-target clippy
with warnings denied, root/workspace tests and the registered simulation suite
(38.642 seconds). Host Cargo runs used `CARGO_INCREMENTAL=0`; existing ignored
tests remain ignored.
Repository formatting/docs checks and `git diff --check` also passed.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet early_recovery_after --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

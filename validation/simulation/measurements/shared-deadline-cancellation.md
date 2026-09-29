# Shared-deadline cancellation boundaries

macOS arm64, based on `cb25ed04a`, 2026-09-27.

## Slice 1: cancellation before expiry

Three callers with staggered admission times share one absolute timeout
deadline. Each loses an alternating request or response during a separate BLE
outage, and the fleet reconnects while all three remain pending. Cancelling the
middle caller before expiry must leave the other two alive until their original
shared deadline. The exact completion map contains only those two survivors;
the cancelled caller emits nothing.

Cancellation immediately reduces the actor count from six to five, reports
`Cancelled` then `NotLive` on a repeat attempt, and changes neither the clock
nor the BLE activity snapshot. Fresh traffic, retained node actors, clock
agreement and full teardown remain checked by the shared fixture.

## Slice 2: cancellation at the deadline tick

The second pair advances virtual time to the common deadline but deliberately
does not poll registered actors there before cancelling the middle caller.
At every earlier step the fleet is drained and no completion is allowed. All
six actors must still exist at the deadline boundary. Cancellation removes
one; draining the remaining actors must return exactly the two surviving
timeouts at that same tick and leave only the three node actors.

The expiry observer now drains directly when already at its target tick,
rather than asking to advance while actors may be ready. The runner correctly
refuses advancement with ready actors; the test helper honors that contract.
Retirement policy and its tests have their own module instead of expanding
the shared outage fixture.

This controls the ordering of caller cancellation versus actor polling. It
does not claim cancellation after the node has processed a timeout, OS-thread
race coverage, or an externally atomic timeout/cancellation operation.

## Coverage and limits

Each slice has two tests, each covering both alternating loss orders, both
request directions, both virtual GATT profile assignments and four schedules
(cyclic, seeds 0, 7, u64::MAX). This adds 64 configurations per slice, with two
three-outage batches each, using the existing eight-actor limit. The 20 prior
flapping cases remain regression controls.

Production impact: none. These are tests of existing production request
behavior over virtual BLE, not firmware or native Bluetooth tests. No private
storage-reclamation, performance, exhaustive scheduling or full byte-replay
claim is made.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet flapping --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
bash validation/hygiene/fmt-docs.sh
git diff --check
```

All 24 focused flapping tests, all-target clippy, root/workspace tests,
formatting/docs checks and `git diff --check` passed. The registered simulation
suite passed in 55.743 seconds. Existing ignored tests remain ignored.
Hardware, firmware and other-host lanes were not run for this host-test-only
pair.

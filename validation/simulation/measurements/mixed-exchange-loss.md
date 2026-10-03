# Alternating request and response loss with overlapping callers

macOS arm64, based on `63820f84b`, 2026-09-27.

## Review slice 2

The repeated-loss fixture now supports two alternating sequences: request,
response, request; and response, request, response. Each wave cuts its selected
packet at an exact header/link-aware partial-frame boundary, then reconnects
while all older callers remain pending. Callers overlap; this does not claim
that request and response fragments occupy both BLE queues simultaneously.

Four tests combine these orders with queued/consumed boundaries. Both request
directions, both CoreBluetooth/BlueZ profile assignments and cyclic plus
version-one seeds 0, 7 and u64::MAX yield 64 configurations, each with two batches
of three losses. The BLE direction being observed switches with the packet
phase, rather than mistaking a request/proof for the selected response.

Every recovery and fresh concurrent response round must precede the first old
deadline. All three older requests then time out separately at their original
start plus 50 milliseconds. Exact completion maps, decreasing actor counts,
fresh payloads between deadlines, frame-local traffic, clocks and full teardown
remain checked. Three nodes, three pending callers and two fresh callers still
fit the original eight-actor budget.

The middle-caller cancellation cases are separate uniform-phase tests; this
slice does not claim their cross-product with alternating loss orders. The
shared fixture moved into a module with a dedicated mixed-order test file.

Production impact: none. Only test scheduling/coverage changes, not shipping
timeout policy, allocation or simulator-library behavior. These are virtual
GATT profiles, not native Bluetooth or hardware. No private retention bound,
exactly-once guarantee, Miri/ISA, benchmark or exhaustive schedule claim is made.

## Verification commands

Host Cargo commands use `CARGO_INCREMENTAL=0`:

All twelve flapping tests passed, including the four new mixed-order tests.
Clippy, root/workspace tests and the registered simulation suite passed
(46.687 seconds). Existing ignored tests remain ignored.
Repository formatting/docs checks and `git diff --check` passed.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet flapping --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

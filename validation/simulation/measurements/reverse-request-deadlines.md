# Reverse-order request deadlines after repeated BLE loss

macOS arm64, based on `ffa01b273`, 2026-09-27.

## Review slice 1

Three requests are admitted at distinct virtual times across repeated BLE
disconnect/reconnect cycles. Their absolute deadlines are batch origin plus
90, 70 and 50 milliseconds, respectively. Each request receives the remaining
duration to its own deadline, not a uniform timeout. The last admitted caller
must expire first; the first admitted caller must remain live until last.

The shared fixture now groups expected completions by explicit absolute
deadline. At each boundary it compares the entire completion map, verifies the
live actor count and confirms retired task IDs cannot be cancelled again.
Every earlier millisecond is checked for premature completion. Fresh concurrent
responses and frame-local traffic succeed between boundaries on the original
logical links. Node actors remain unchanged and teardown checks still apply.

Four tests combine alternating request/response/request and
response/request/response loss with queued/consumed partial-frame boundaries.
Two request directions, two virtual GATT profile assignments and cyclic plus
seeded schedules (0, 7, u64::MAX) yield 64 configurations, each repeated for two
batches. All recovery occurs before the earliest deadline. The previous
admission-order and middle-cancellation cases retain their original semantics.

Production impact: none. This changes test coverage and fixture deadline
bookkeeping only. No native Bluetooth, hardware, private retention bound,
exhaustive scheduling, performance or full byte-replay claim is made.

## Verification

Host commands use `CARGO_INCREMENTAL=0`. All 20 flapping tests and simulation
all-target clippy passed. Broader verification is recorded with the companion
[shared-deadline slice](shared-request-deadline.md).

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet flapping --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
```

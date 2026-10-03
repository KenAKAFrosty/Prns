# Held tail buffers across transfer watchdog expiry

Local macOS arm64 candidate based on `47d048949`, 2026-09-26.

## Scope and finding

This slice is test-only. No production behavior, retained state, capacity or wire
format changes.

The initial attempt to reuse the between-segment request-expiry helper failed
because there is no armed request deadline after continuation admission. This is
intentional: Resource transfer watchdogs own timeout while that transfer is live.
The final tests therefore advance through actual earliest Resource deadlines,
without changing retry counters, injecting deadlines or removing receipts.

Two deterministic core-engine cases hold the competing whole response's second
open job while the legitimate split continuation is admitted but receives no data.
One returns copied bytes; the other detaches the growable-heap transfer buffer
before driving time forward. Every watchdog retry frame must be a ResourceRequest.
The bounded watchdog loop eventually requires the whole capture to contain exactly:

- `OpenTimedOut` for the held whole response.
- `RetriesExhausted` for the split continuation.
- One request settlement: `ResponseTransferFailed(RetriesExhausted)`.

Failure order is not prescribed; the exact failures and sole settlement are.
Neither transfer nor the split assembly or request receipt survives. Returning the
old copied/detached worker result must produce no effects and consume no entropy.
Delivering the delayed continuation afterward also produces no effects. Incoming
transfer storage and its active buffer accounting remain empty.

Unlike success-before-return cases, the whole worker's own deadline has already
retired its slot here, so late return is inert rather than emitting cancellation.
The sender watchdog is not driven in this fixture. These are core-engine tests,
not new mixed-runtime or threaded-worker scenarios.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`. Passed:

- `cargo test --locked -p prns-core --lib streamed_tail_cannot_revive_an_expired --quiet` (two tests).
- `cargo test --locked -p prns-core --lib split_admission_tests --quiet` (24 tests).
- `cargo test --locked -p prns-core --no-default-features --features std --lib streamed_completion --quiet` (five tests, without optional worker offloading).
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `python3 validation/run.py run --suite virtual-device-simulation`, including
  the existing 56 mixed-runtime BLE scenarios, not the new core-engine fixtures.
- `bash validation/hygiene/fmt-docs.sh`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `git diff --check`.

No production mutation was used in this slice. Firmware, ISA/Miri, physical
hardware, stock interoperability and benchmarks were not rerun.

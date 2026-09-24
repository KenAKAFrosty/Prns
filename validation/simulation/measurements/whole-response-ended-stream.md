# Streamed workers after their request ends

Local macOS arm64 candidate based on `858f92c86`, 2026-09-26.

## Scope

Test-only assurance of the shared-core ended-request claim check. No production
behavior, retained state, capacity or wire format changes.

Four new deterministic core-engine cases extend the existing authentic-frame
fixture:

- Return the held first ciphertext span after successful split completion.
- Return it after the actual between-segment request deadline expires.
- Return a copied tail span after successful split completion.
- Return a detached growable-heap tail buffer after successful split completion.

The first-span cases finish the remaining ciphertext inline and assert exactly
one worker completion. The tail cases admit the split continuation before returning
the first span, forcing a second worker job. They hold that second result while the
original split completes through actual frames, then return it. The detached case
takes the transfer buffer out of core storage before completing the split; duplicate
take and dispatch remain forbidden. Thus the request ends while the worker owns
the buffer, not merely before a later detachment.

Every ended case requires only authenticated cancellation of the competing whole
response, no revived request receipt, and zero retained incoming transfers or
buffer bytes. Successful split completion first asserts its exact final chunk and
single success settlement; expiry first asserts its single timeout. Existing
active-split cases still complete the original response after cancellation.

The boundary helper now accepts both fixed and growable storage, and can complete
an already-admitted continuation without admitting it twice. Virtual timestamps
advance monotonically. No assembly or receipt state is manually removed.

These are core-engine fixtures, not new mixed-runtime or threaded-worker scenarios.
Timeout while a tail buffer is detached and a continuation is in flight remains
separate coverage work; the timeout here occurs between split segments.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`. Disabling only the absent-request predicate
made all four new tests fail on unexpected publication; both existing active-split
tests still passed. Restoring the predicate passes. No production mutation remains.

Passed:

- `cargo test --locked -p prns-core --lib streamed_ --quiet` (11 tests).
- `cargo test --locked -p prns-core --lib split_admission_tests --quiet` (22 tests).
- `cargo test --locked -p prns-core --no-default-features --features std --lib streamed_completion --quiet` (four tests, without optional worker offloading).
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `python3 validation/run.py run --suite virtual-device-simulation`, including
  the existing 56 mixed-runtime BLE scenarios, not the new core-engine fixtures.
- `bash validation/hygiene/fmt-docs.sh`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `git diff --check`.

Firmware, ISA/Miri, physical hardware, stock interoperability and benchmarks were
not rerun for this test-only slice.

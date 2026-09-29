# Whole-worker verdicts after their request ends

Local macOS arm64 candidate based on `50b1e2568`, 2026-09-26.

## Scope

Test-only assurance of the preceding shared-core ended-claim fix. No production
behavior, retained state, capacity or wire format changes.

The existing deterministic core-engine fixture now crosses three request
boundaries (active split, successful split completion, deadline expiry) with three
whole-open verdicts (`Opened`, `OpenedAndDigested`, `Unavailable`). All nine cases
use an actual issued worker job and authenticated response bytes. Success and
expiry use the same boundary helper as the direct/decompression tests: real
continuation delivery or the request's actual timeout, with exact terminal effects.
The external worker lane is restored before returning the delayed verdict, so the
unavailable case exercises its real inline fallback.

Each late verdict must emit only authenticated cancellation, retire the transfer,
leave the request deadline unchanged (absent after success/expiry), and preserve
the external worker lane. Replay must be `Stale`, with neither effects nor entropy
consumption. The active case additionally replays after continuation slot reuse
and proves exact split completion; the ended cases do not claim that slot-reuse
coverage. Separate named tests make both ended boundaries independently runnable.

These are deterministic core-engine fixtures, not new mixed-runtime simulations
or threaded worker-pool tests. Ended-claim streamed and detached-buffer variants
remain separate coverage work.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`. Disabling only the absent-request predicate
made both new named tests fail on unexpected publication while the active-split
test still passed. The predicate was restored; no production mutation remains.

Passed:

- `cargo test --locked -p prns-core --features resource-work-offload --lib delayed_whole_open_verdicts --quiet` (three tests, nine cases).
- `cargo test --locked -p prns-core --features resource-work-offload --lib split_admission_tests --quiet` (18 tests).
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

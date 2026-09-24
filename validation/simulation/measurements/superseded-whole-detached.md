# Detached worker buffers during response overlap

Local macOS arm64 candidate based on `7df4c52b7`, 2026-09-26.

## Scope and assertions

Test-only follow-up to the streamed-open overlap scenario. No production behavior,
firmware storage, capacities or wire formats change. The split-response fixture
now supports an explicit storage type while existing tests retain their original
fixed-capacity type. The streamed scenario is reused with `GrowableHeap` and a
detached-tail workspace, keeping the admission sequence and exact response
assertions shared with the copied-span cases.

The older whole response's first open job is held while all data parts arrive.
After the split's first segment completes and its continuation is admitted, the
held job returns. Real transfer overlap causes core to dispatch the remaining
span with a transferable reservation. The test requires `DetachedTransfer`,
asserts the row is nonresident, and refuses a second workspace take or duplicate
dispatch. It compares the reserved ciphertext span with the captured job bytes,
decrypts that span in the owned buffer, and returns it as `ReturnedTransfer`.
The fixture intentionally retains a copy for comparison; no allocation-count or
zero-copy performance claim is made.

Landing the owned buffer must emit only the authenticated competitor cancellation,
preserve the original request deadline, settle the competitor as `RejectedByPeer`,
and allow exact final split delivery and one request success with clean storage.
This is a deterministic core-engine test, not a threaded worker-pool, mixed-runtime
BLE or hardware test. Completion after split ownership ends and conflicting-peer
mixed-runtime injection remain separate coverage work.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`. Focused `streamed_` tests pass, including
the existing copied-span cases and new detached-tail case. Temporarily bypassing
the existing `Ok(_) if superseded` conclusion guard makes
`cargo test --locked -p prns-core --lib detached_streamed_tail --quiet` fail on
unexpected publication. Restoring the guard passes; no production mutation remains.

`cargo test --locked -p prns-core --lib split_admission_tests --quiet` passes all
16 tests after the fixture generalization. Firmware, ISA/Miri, physical hardware,
interoperability and benchmarks are not rerun for this test-only slice.

Additional passed checks:

- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked -p prns-core --no-default-features --features std --lib delayed_streamed_open_cannot --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`, including
  the existing 56 mixed-runtime BLE scenarios.
- `bash validation/hygiene/fmt-docs.sh`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

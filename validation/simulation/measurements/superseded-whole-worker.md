# Superseded whole-response worker verdicts

Local macOS arm64 candidate based on `6cbb9dc6e`, 2026-09-26.

## Scope

This is test-only assurance of the preceding shared-core completion fix. No
production behavior, firmware storage, capacities or wire formats change.

The deterministic core-engine fixture captures an actual `WholeResourceOpen`
job emitted by a whole response before the competing split response is admitted.
It authenticates/decrypts the captured bytes using the issued plan and computes
the Resource proof. After receiving and verifying the first split segment, it
returns each of these worker verdicts:

- `Opened`, requiring core to verify the stream.
- `OpenedAndDigested`, carrying the calculated hash and proof.
- `Unavailable`, requiring the existing inline fallback.

Each must emit exactly one authenticated cancellation, settle the competitor as
`RejectedByPeer`, preserve the original request deadline, retire the superseded
transfer, and preserve the configured external-open lane. Replaying the old job
must return `Stale` without effects or entropy consumption, both with empty
transfer storage and after admitting the original split's continuation. Finally,
that continuation must deliver the exact final chunk and exactly one success.

The test switches the open lane only to let the split fixture run normally while
holding the already-dispatched competitor job. It restores `ExternalWhole` before
applying the delayed verdict, including the unavailable-worker fallback. No
assembly state is manually installed and virtual time advances monotonically.

## Streamed-open follow-up

A second fixture holds the first real `ResourceOpen` span while all four parts
of the whole response arrive. It then admits and verifies the split's first
segment. Returning the held span between segments permits inline completion of
the remaining ciphertext; returning it while the split continuation is receiving
dispatches a second worker span. The test asserts these exact one/two completion
paths, processes actual ciphertext with `StreamedOpen::chew_span`, and checks the
same authenticated cancellation, unchanged deadline and exact split completion.

This is not a threaded worker-pool or mixed-runtime BLE test. Streamed spans use
copied returned bytes, not detached transfer buffers. Detached-transfer ownership,
late arrivals after split ownership ends and mixed-runtime conflicting-peer
injection remain separate coverage work.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`.
The focused command `cargo test --locked -p prns-core --lib
delayed_whole_open_verdicts --quiet` passes. Temporarily bypassing the existing
`Ok(_) if superseded` conclusion guard makes it fail on unexpected publication;
restoring the guard passes. No production mutation remains in the slice.
The streamed-open test also fails on unexpected publication with that guard
disabled, and passes after restoration.

Passed:

- `cargo test --locked -p prns-core --lib delayed_streamed_open_cannot --quiet`.
- `cargo test --locked -p prns-core --no-default-features --features std --lib delayed_streamed_open_cannot --quiet` (without optional worker offloading).
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `python3 validation/run.py run --suite virtual-device-simulation`, including
  the existing 56 mixed-runtime BLE scenarios.
- `bash validation/hygiene/fmt-docs.sh`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

Firmware, ISA/Miri, physical hardware, interoperability and benchmarks are not
rerun for this test-only slice.

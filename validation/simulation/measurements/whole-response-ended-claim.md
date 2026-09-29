# Whole responses after their request ends

Local macOS arm64 candidate based on `657a28eb2`, 2026-09-26.

## Finding and shared-core fix

The preceding ownership checks protected an active split assembly. Once that
assembly and its request receipt were gone, a valid pre-admitted whole response
could instead receive a proof and fall through to unsolicited `ResourceReceived`
delivery. The expanded deterministic core-engine test reproduced the proof where
cancellation was required after successful split completion.

Completion now requires a live matching request receipt as well as freedom from
a competing split owner. One private shared-core predicate serves normal completion
and the delayed decompression boundary. An absent claim cancels and retires the
whole transfer without proof, delivery or another request settlement. Admission
already rejects response advertisements without a matching pending request; this
enforces that invariant again after potentially asynchronous transfer work.
Requests and genuinely unsolicited Resources keep their existing delivery paths.
There is no new retained state, capacity or wire format.

## Exact coverage and limits

The existing authentic-frame fixture now runs six cases: uncompressed or delayed
decompression completion while the split is active, after successful final-segment
delivery, and after its between-segment request deadline expires. Success and expiry
are driven through core APIs and asserted as whole captures, not by manually
deleting the assembly. Time advances monotonically and the expiry timestamp is
derived from the actual request deadline.

Every competitor receives exactly one authenticated cancellation and settles as
`RejectedByPeer`, with no extra delivery or request settlement and reclaimed
transfer storage. The active case still finishes the exact original split. The
completed case first proves the exact final chunk and terminal success; the expired
case first proves one named `Timeout`. Neither permits late whole completion to
reclassify response bytes as unsolicited application data.

This is not yet a mixed-runtime conflicting-peer scenario. Ended-claim external
worker and detached-buffer variants remain additional coverage work; their active
ownership cases are covered by preceding slices. No full overlap-matrix claim.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`; canonical firmware builds do not override
their environment. The expanded `preadmitted_whole_completion` test fails before
the fix and passes after it; the full core suite passes. Passed commands:

- `cargo test --locked -p prns-core --lib preadmitted_whole_completion --quiet`.
- `cargo test --locked -p prns-core --quiet`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- Registered `virtual-device-simulation` suite, including 56 mixed-runtime BLE
  scenarios (not the new conflicting-peer ended-claim fixture).
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- Registered `embedded-miri-quick`, `embedded-isa-thumbv7em`,
  `embedded-isa-riscv32imac` and `embedded-isa-xtensa-esp32s3` suites. Their existing
  assurance scenarios do not directly execute the new ended-claim fixture.

A targeted mutation disabling only the absent-request condition reproduces
`ResourceProof` instead of cancellation in the focused test. The condition was
restored; no mutation remains in the candidate.

`./tools/prns build embedded resources report --target t114` passed: image
625,052 bytes (-376), static RAM 142,996 bytes (unchanged), RAM headroom 360 bytes
(unchanged), modeled stack reservation remainder 10,352 bytes (unchanged), flash
headroom 140,900 bytes. Stack evidence retains its indirect-call/interrupt gaps.
This is a size observation, not a performance claim.
`./tools/prns build embedded resources contracts --check` still fails on the
pre-existing stale `tools/release/flasher_memory_contracts.py`, which is untouched.

Doctor reports missing Renode and about 0.5 GiB free disk, below the full resource
matrix requirement. No full firmware matrix, physical hardware, stock
interoperability or benchmark rerun is claimed.

# Completion of a superseded whole response

Local macOS arm64 candidate based on `f4aa9f64a`, 2026-09-26.

## Finding and production change

The simulator-led ownership investigation now covers a valid whole response
admitted before a split response acquired the same link/request. Before this
slice, the core-engine reproduction emitted `ResourceProof` instead of rejecting
the competing response. The prior delivery path could publish that response and
settle the request while its split response was still incomplete.

Shared core now checks ownership after verification and before proof/publication.
It retires the superseded transfer and emits an authenticated receiver cancellation
using fresh caller-provided entropy. Normal completion reports the named
`SupersededResponse` outcome; delayed decompression refreshes the affected wake
schedules. Neither path mutates the owning split assembly or its receipt deadline.
Invalid transfers retain their typed failure path, protected by the preceding
failed-claim ownership fix. No new retained state, capacities or wire formats.

## Deterministic coverage

Independent sender engines share test link keys to bypass sender-side `LinkBusy`.
Both cases admit the whole response first, then receive and verify the first split
segment through real protocol frames. One releases the whole response's data part;
the other parks decompression before the split starts and returns valid plaintext
after the split's first segment. The decompression seam is supplied plaintext
directly; this is not a bzip2 codec test.

Exact assertions require one cancellation, no competing delivery or request
settlement, competitor `RejectedByPeer`, unchanged original deadline, released
transfer storage, and the original final split chunk plus exactly one success.
The shared split fixture can now start from an already pending request and checks
that unrelated retained transfer rows survive its first-segment setup.

The focused `preadmitted_whole_completion` test failed before implementation on
the proof-versus-cancellation distinction. After both guards were added it passed.
Temporarily disabling only the decompression guard failed on unexpected delivery;
restoring it passed. This targeted mutation checks an independently reproduced
ownership requirement, not a performance optimization.

These are core-engine tests, not mixed-runtime inconsistent-peer injections.
External whole-open and streamed-open verdicts converge on the guarded conclusion
path, but explicit delayed-verdict overlap scenarios remain to be added. Arrivals
after split ownership ends are also outside this slice's assertions.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`; canonical firmware builds do not override
their environment. Passed:

- `cargo test --locked -p prns-core --lib preadmitted_whole --quiet` and
  `cargo test --locked -p prns-core --quiet`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- Registered `embedded-isa-thumbv7em`, `embedded-isa-riscv32imac` and
  `embedded-isa-xtensa-esp32s3` suites (existing assurance scenarios).
- Registered `virtual-device-simulation` and `embedded-miri-quick` suites;
  the Miri scenarios do not directly execute the new overlap test.
- `bash validation/hygiene/fmt-docs.sh`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

`./tools/prns build embedded resources report --target t114` passed: image
625,428 bytes (+1,016), static RAM 142,996 bytes (unchanged), RAM headroom 360 bytes
(unchanged), modeled stack reservation remainder 10,352 bytes (unchanged), flash
headroom 140,524 bytes. The stack model retains its indirect-call/interrupt gaps.
`./tools/prns build embedded resources contracts --check` still fails on the
previously stale `tools/release/flasher_memory_contracts.py`, which is untouched.

Doctor reports approximately 0.3 GiB free disk and missing Renode. No full firmware
matrix, physical hardware, stock interoperability or benchmark run is claimed.

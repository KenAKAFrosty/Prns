# Failure of a superseded whole response

Local macOS arm64 candidate based on `72027cd91`, 2026-09-26.

## Finding and production change

This continues the simulator-led whole/split response ownership investigation.
It is a deterministic core-engine reproduction, not a new mixed-runtime BLE
injection. Independent sender states share test link keys to bypass the normal
sender's `LinkBusy` safeguard while producing authenticated protocol frames.

An older whole response can already be receiving when a split response acquires
the same link/request. Previously, failure cleanup for the whole transfer settled
that request despite the split assembly still owning it. The pre-fix tests observed
an unexpected `SendRequest(Err(ResponseTransferFailed(MetadataOverrun)))`.

Three production lines reuse the existing shared-core ownership predicate before
failed-claim settlement. The failed transfer is still retired and its typed
Resource failure is still reported; only the unrelated request settlement is
suppressed. No storage fields, capacities, wire formats or adapter policy are added.

## Coverage

The new fixture admits the whole response, admits and completes the first split
segment through protocol frames, then fails the older whole response through each
of malformed metadata, authenticated sender cancellation and retry exhaustion.
It checks the exact failure without request settlement, unchanged split identity
and request deadline, retired transfer storage, and exact final split chunk plus
one successful request settlement. Both split segments are verified by core;
assembly ownership is not manually installed in this fixture.

The existing synthetic ownership-boundary fixture additionally checks malformed
metadata through uncompressed opening and the decompression-completion seam. The
latter supplies worker plaintext directly, not a bzip2 codec implementation.

The focused command `cargo test --locked -p prns-core --lib preadmitted_whole
--quiet` failed both tests before the guard and passes after it. This is also the
exact guard-omission comparison: the only production delta is the three-line
guard. The rationale is request ownership, not improving a mutation score.

Successful completion of an older whole transfer, including a delayed successful
worker verdict, remains a separate gap. This slice does not claim exhaustive
whole/split arbitration or inconsistent-peer mixed-runtime coverage.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; the canonical firmware builder
does not receive that override.

- Focused `preadmitted_whole` tests and `cargo test --locked -p prns-core --quiet`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet` passed.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings` passed.
- `cargo check --locked -p prns-core --no-default-features` passed.
- `python3 validation/run.py run --suite virtual-device-simulation` passed.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards` passed.
- Registered `embedded-miri-quick`, `embedded-isa-thumbv7em`,
  `embedded-isa-riscv32imac` and `embedded-isa-xtensa-esp32s3` passed. These suites
  exercise their existing scenarios, not the new failure fixture directly.

`./tools/prns build embedded resources report --target t114` passed: image
624,412 bytes (+128), static RAM 142,996 bytes (unchanged), RAM headroom 360 bytes
(unchanged), modeled stack reservation remainder 10,352 bytes (unchanged), flash
headroom 141,540 bytes. Stack evidence retains its indirect-call/interrupt gaps.

`./tools/prns build embedded resources contracts --check` still fails on the
pre-existing stale `tools/release/flasher_memory_contracts.py`; it is unchanged.

Doctor reports missing Renode and 0.4 GiB free disk, below the full resource
matrix requirement. No full board matrix, physical hardware, stock interoperability
or benchmark rerun is claimed.

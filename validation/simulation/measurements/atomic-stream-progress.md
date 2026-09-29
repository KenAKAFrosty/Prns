# Atomic split-stream progress

Local macOS arm64 candidate based on `ca3af4a22`, 2026-09-26.

## Finding and scope

The receiver already checked cumulative stream sizes before proof and delivery,
but `IncomingAssemblies::advance` independently used saturating addition and
accepted incomplete final totals. A new owner test reproduced completion at 255
bytes for a 256-byte declaration. This is a shared-core API audit following the
simulator-led response-accounting work, not a newly reproduced runtime wire bug.

Validation and commit now use one checked calculation. Advancement refuses
overflow, overrun and a final total unequal to the original declaration, without
changing either the byte counter or segment position. The pre-delivery check
remains necessary: a refusal after publishing bytes would be too late. Both
fixed and heap storage use the same rule.

There are no added retained fields, buffers, allocation paths or wire changes.
Current receiver behavior should remain unchanged because it already makes the
pre-delivery check; the production improvement is removing the assembly API's
dependence on callers remembering that check. Delivered-value budgets, excluding
metadata and response framing, remain the next accounting task.

## Coverage

Fixed and heap owner tests compare validation and commit with an independent
`u128` arithmetic oracle. Cases include exact bounds, final underrun, intermediate
and final overrun, zero, `u64::MAX`, and overflow at that maximum. A refused update
is followed by a valid update at the same position with the exact remaining byte
count, proving that neither stored counter changed.

Existing completion fixtures now supply the full declared total. Identity tests
use otherwise-valid byte counts so size refusal cannot mask a missing identity
check. Random chain-position tests likewise use valid-size candidates; the new
property matrix separately exercises arbitrary `u64` byte counts.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; the canonical firmware builder
retains its own environment. Passed:

- `cargo test --locked -p prns-core --lib routing::links::resources::assembly --quiet`:
  17 owner tests.
- `cargo test --locked -p prns-core --quiet`: 2,036 library tests passed,
  three existing tests ignored, plus the other package targets.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`:
  all six passed, including 52 mixed-runtime BLE tests.
- `bash validation/hygiene/fmt-docs.sh`, including registries and documentation
  links, and `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`: 142,988 static
  RAM bytes, 368 bytes RAM headroom and 10,336 bytes estimated stack remaining,
  all unchanged. Flash headroom is 144,100 bytes, a 40-byte code-size increase.
  Existing stack-analysis gaps remain. The first run correctly refused evidence
  because test/documentation edits overlapped its snapshot; the stable rerun passed.

Focused manual mutation audit: temporarily remove final-size equality, then
separately replace checked addition with saturating addition in
`next_stream_total`. Each compiled and failed
`cargo test --locked -p prns-core --lib stream_progress_rejects_invalid_sizes --quiet`.
Both edits were restored. The initial red test separately demonstrates that
bypassing validation during commit is caught. This is targeted diagnostic
evidence, not a cargo-mutants score or exhaustive proof.

The assurance doctor still reports insufficient matrix disk space (about 0.8 GiB
free versus 24 GiB required) and missing Renode. Generated-contract checking still
reports the pre-existing stale `tools/release/flasher_memory_contracts.py`, which
is untouched. The full board matrix, platform pilots, hardware and a new benchmark
were not run. Existing simulator and Miri/ISA suites are regression evidence, not
direct embedded execution of these new assembly owner tests.

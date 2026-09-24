# Verified split-response value accounting

Local macOS arm64 candidate based on `ae43ac985`, 2026-09-26.

## Scope and production effect

Assemblies now retain separate stream and delivered-value totals. `AssemblyBytes`
requires both lengths at advancement; there is no implicit singleton byte count.
The transition checks both additions and refuses a value delta larger than its
stream delta before mutating either counter. Fixed and heap tables preserve the
value total through replacement, reuse and swap removal.

A private split-delivery module removes verified first-segment response framing,
then checks the cumulative value budget before publishing a chunk. Metadata-bearing
files remain literal, including envelope-shaped bytes. Later segments are literal
continuations. Normal opening and worker-decompression completion share the same
check. Refusal settles `ResponseTooLarge` and releases the receiver's transfer and
assembly without publishing the offending chunk or successful partial result.

This is accounting infrastructure, not yet relaxed acceptance. The conservative
split-advertisement stream limit is deliberately unchanged. Current valid traffic
already fits that stricter bound, so it should behave unchanged; the new value
counter and verified check establish the contract needed to remove it safely.
There is one additional `u64` per assembly, plus heap-column bookkeeping, with no
buffer or queue capacity changes.

## Simulator finding that limits this slice

Temporarily relaxing the advertisement check made the exact-value owner test pass,
but all 27 selected segmented BLE scenarios failed their common recovery prelude:
the receiver settled its zero-budget request while the responder did not report
`RejectedByPeer`. The late refusal did not emit an encrypted cancellation, leaving
the sender's transfer live. The existing completion paths lack the entropy input
needed for that cancellation. This was a candidate regression caught by the
simulator, not evidence of that failure in the committed baseline.

The admission relaxation was removed. Completion retains its existing proof
ordering. The next slice must plumb fresh entropy/cancellation through ordinary,
worker-open and decompression completion, test prompt sender release and reuse,
then enable exact-value admission. Do not merely loosen the offer check or add
timeout waits to the recovery tests.

## Owner coverage

Fixed/heap property tests compare cumulative budgets against a `u128` oracle,
including zero, unlimited and overflow. Lifecycle tests cover row removal, reuse,
replacement and atomic refusal of impossible value deltas.

Encrypted core-engine fixtures cover ordinary and worker-decompressed segments,
legacy raw bodies, response envelopes, metadata files and literal envelope-shaped
files. Zero, first-chunk-only, one-byte-short, exact, maximum and unlimited bounds
are checked, including MessagePack-header-shaped bytes. Refusal retains only
previously published provisional chunks, settles once, retires receiver storage,
and ignores replayed parts. Buffered API discard behavior remains covered by the
existing simulator scenarios, not by these direct-journal fixtures.

To isolate the new conclusion check while admission remains strict, the fixtures
explicitly replace the receipt policy after each advertisement is admitted. This
is synthetic test setup, not a public policy-edit operation or proof of exact-limit
end-to-end acceptance. The inflate fixture supplies worker output, not a bzip2 codec.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; the firmware builder retains its
canonical environment. Passed:

- `cargo test --locked -p prns-core --quiet`: 2,040 library tests passed,
  three existing tests ignored, plus the other package targets.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`:
  all six passed, including the 52 mixed-runtime BLE tests and their recovery preludes.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`: static RAM
  142,996 bytes (+8), RAM headroom 360 bytes, estimated stack remaining 10,352
  bytes (+16), and flash headroom 143,636 bytes (464 bytes more code).
  Existing stack-analysis gaps remain; no capacity or guard was reduced.

Focused manual mutation audit replaced the value counter with the stream counter,
then separately ignored the prior value total. Both compiled and failed
`cargo test --locked -p prns-core --lib split_response_value_limits --quiet`.
The first wrongly charged framing; the second admitted an oversized aggregate
whose individual chunks fit. Both edits were restored. This is targeted diagnostic
evidence, not an exhaustive mutation score.

The assurance doctor still reports about 0.8 GiB free versus the 24 GiB matrix
requirement and missing Renode. Generated-contract checking still fails for the
pre-existing stale `tools/release/flasher_memory_contracts.py`, left untouched.
The full board matrix, platform pilots, hardware and a new benchmark were not run.
Existing Miri/ISA suites are regression evidence, not direct execution of these new
value-accounting tests on embedded instructions.

# Split-stream size identity

Local macOS arm64 candidate based on `ee8b40b97`, 2026-09-26.

## Finding and production change

The preceding cumulative-size check compared actual stream bytes with the current
segment's declaration. It did not retain the first declaration. A new encrypted
core-engine test demonstrated that a continuation could change that total and
still be admitted. This is an audit follow-up to the simulator-led accounting
work, not a new mixed-runtime simulator injection.

Each incoming assembly now stores its original stream size in one additional
`u64` column. Both fixed and heap implementations retain it through replacement,
reuse and swap removal. `begin` and the table's `push` require the size explicitly;
there is no inferred or defaulted compatibility overload. `fit` and `advance`
take the existing typed `ResourceSegment` description, requiring index, count and
total bytes together with chain hash and semantic correlation.

Changed-size continuations are ignored before response policy, allocation or
receipt claiming. The original request remains able to complete with a correct
continuation. Queued promotion and refusal also revalidate size identity. Normal
opening and resumed decompression reject transfers whose assembly size has since
changed, without settling or clearing the replacement chain that owns the same
request. Cumulative stream validation independently requires the stored size.

Wire shapes and buffer capacities are unchanged. There is a real storage cost:
eight bytes per fixed assembly slot, and eight bytes per heap row plus its new
vector's bookkeeping. The T114 measurement below establishes one target's compiled
cost, not every board's. Final delivered-value accounting remains separate from
stream bytes, which still include metadata and response framing.

## Coverage

- An encrypted continuation test changes the total to zero, one byte smaller,
  one byte larger and `u64::MAX`. Each is ignored without claiming the receipt;
  the original continuation then succeeds exactly.
- Existing queued promotion and expiry matrices now include a changed-size row.
- Both ordinary opening and the worker-decompression seam preserve a replacement
  assembly and receipt while refusing the stale admitted transfer. The inflate
  fixture supplies worker output rather than exercising a bzip2 codec.
- Property and lifecycle tests exercise fixed and heap size identity, including
  zero/maximum values, failed advancement, replacement, reuse and swap removal.
- The older size-refusal cleanup fixture now explicitly re-registers the receipt
  with a tighter policy while retaining the assembly. A rewritten continuation
  declaration would now be ignored before that refusal path; the synthetic policy
  setup keeps that cleanup branch covered without claiming a public policy editor.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; canonical firmware builds retain
their own environment. Passed:

- `cargo test --locked -p prns-core --quiet`: 2,034 library tests passed, three
  existing tests ignored, plus the other package targets.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`:
  all six suites passed, including the 52 mixed-runtime BLE tests.
- `bash validation/hygiene/fmt-docs.sh` (including registry checks), and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`: 142,988 static
  bytes plus four bytes padding, 69,632 bytes runtime reservation and 368 bytes
  RAM headroom. Static RAM grew eight bytes. Stack evidence leaves 10,336 bytes,
  sixteen fewer than before, with existing unresolved-call/frame gaps. Flash
  headroom is 144,140 bytes, 552 fewer than before. No throughput claim is made.

Focused manual mutation audit: temporarily remove the stream-size equality in
`IncomingAssemblies::fit`, then separately invert it, restoring the source after
each run of `CARGO_INCREMENTAL=0 cargo test --locked -p prns-core --lib stream_size --quiet`.
Both mutations compiled and were caught. Removal fails four tests, including
changed-size admission and protection of the replacement request during stale
completion. Inversion fails six, including valid-stream controls. This is targeted
diagnostic evidence, not an exhaustive mutation score.

The assurance doctor still reports about 0.9 GiB free versus the 24 GiB matrix
requirement and missing Renode. Generated-contract checking still refuses the
pre-existing stale `tools/release/flasher_memory_contracts.py`; it and its inventory
inputs are untouched. Full board builds, platform pilots and hardware tests were
not run. Existing simulator and Miri/ISA suites are regression coverage, not direct
execution of these new malformed-peer cases on firmware or physical radios.

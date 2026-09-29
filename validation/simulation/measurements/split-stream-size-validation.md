# Cumulative split-stream validation

Local macOS arm64 candidate based on `f93f2058e`, 2026-09-26.

## Motivation and change

The simulator-led response accounting work needs a trustworthy cumulative stream
boundary before relaxing the existing conservative admission policy. Previously,
individual segment hashes were verified but a split chain's combined stream
length was not checked against its advertised size. New core-engine reproductions
failed on both normal opening and resumed decompression: the receiver emitted a
proof for a segment despite a false stream size.

The shared assembly owner now checks the already-received stream count plus the
current verified segment using checked arithmetic. The sum must not exceed this
segment's advertised total; the final segment must equal it. It also requires
the next position and expected segment count. Existing separate chain-identity
checks still precede conclusion. A bad size returns `TransferCorrupt` before a
proof or delivery of the offending segment, and existing failure settlement
retires the transfer, receipt and assembly. Earlier raw chunks remain provisional.

Normal opening (including external-open completion) and the decompression
completion seam use the same predicate. The existing counter measures stream
bytes, including metadata and response framing, not delivered application bytes.
There are no new stored fields, buffers, capacities or wire shapes. The current
conservative response-size admission policy is unchanged.

This deliberately does not claim the original size declaration is immutable:
continuations still carry their own declared total. Binding that first declaration
across the chain and enforcing cumulative delivered-value limits remain follow-ups.
Likewise, this check is after opening/inflation; existing allocation ceilings
remain responsible for protection before those operations.

## Tests and scope

Two encrypted core-engine tests cover normal and resumed-inflate completion with
advertised lengths of zero, one less than the first segment, exactly the first
segment, one less than the full stream, the exact full stream, and one greater.
Invalid cases require no proof or offending chunk, exact failure settlement,
retired state and no delivery on replay. Exact-size controls require both complete
chunks and exact terminal RTT. The inflate fixture supplies worker output; it
does not test a bzip2 codec.

A property test compares the predicate with independent `u128` arithmetic and
checks non-mutation and unknown-link refusal. A focused integer-limit test rejects
overflow under a maximum `u64` advertisement while allowing the exact maximum.
These are core-engine/table tests, not new mixed-runtime fault injections.
The existing split failure tests now own a private child module for these cases.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; the canonical resource builder does
not override its environment. Passed:

- `cargo test --locked -p prns-core stream_sizes --quiet`: both red/green engine
  regressions pass after the change.
- `cargo test --locked -p prns-core --quiet`: 2,030 library tests passed, three
  existing tests ignored, plus the remaining package targets.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy --suite embedded-miri-quick --suite embedded-isa-thumbv7em --suite embedded-isa-riscv32imac --suite embedded-isa-xtensa-esp32s3`:
  all six suites passed, including the 52 mixed-runtime BLE tests.
- `bash validation/hygiene/fmt-docs.sh` (including registry checks) and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `./tools/prns build embedded resources report --target t114`: static RAM
  142,980 bytes plus four bytes padding, runtime reservation 69,632 bytes and
  headroom 376 bytes, unchanged. Existing stack evidence leaves 10,352 bytes with
  its unresolved-call/frame gaps. Flash headroom is 144,692 bytes, 728 fewer than
  the preceding slice. No speed claim follows from that code-size change.

Focused mutation audit:

```console
CARGO_INCREMENTAL=0 cargo mutants --no-config --in-place -p prns-core \
  --file prns-core/src/routing/links/resources/assembly/core.rs \
  --re 'fits_stream_size -> bool with|core.rs:150:22:|core.rs:151:31:.*< with <=' \
  --timeout 60 --build-timeout 180 \
  --output validation-artifacts/mutation/split-stream-size -- --lib stream_size
```

Baseline passed and all four selected mutants were caught: always accepting,
always refusing, reversing the cumulative bound, and allowing the final segment
to bypass exact equality. No survivors or timeouts. This is a focused diagnostic
audit, not exhaustive mutation coverage.

The assurance doctor still reports insufficient disk (about 0.9 GiB versus the
24 GiB matrix requirement) and missing Renode. Generated-contract checking still
refuses the pre-existing stale `tools/release/flasher_memory_contracts.py`;
neither it nor its inventory inputs changed. The full firmware matrix, platform
pilots and physical hardware were not run. Existing simulation and Miri/ISA runs
are regression evidence, not direct target execution of the new malformed-size
injections.

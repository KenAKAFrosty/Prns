# Split-response budgets and completion cancellation

Local macOS arm64 candidate based on `3ba74151a`, 2026-09-26.

## Production behavior

Split Resource responses now apply request limits to cumulative verified value
bytes, not the advertised stream including framing. Independent stream-size,
transfer-storage and queue bounds remain. Exact-value budgets accept framed
responses; zero and one-byte-short budgets fail before publishing the offending
chunk. Previously published chunks remain provisional until terminal success.

A completion-time refusal emits the existing authenticated receiver cancellation,
not a success proof. Both peers promptly retire their transfers and assemblies;
the receiver settles `ResponseTooLarge` and the responder `RejectedByPeer`.
This closes the candidate regression found by the simulator when the preceding
slice experimentally relaxed admission. No wire formats, buffer capacities or
retained state change in this slice.

Ordinary open, external whole-open and decompression completion now require an
explicit fresh-entropy callback. Tokio, Embassy, WASM and the benchmark adapter
all forward their existing entropy source. There is no stored nonce or permissive
default. Existing whole-response proof ordering is unchanged.

## Evidence

The encrypted owner-test matrix now installs the actual limit at request creation,
without synthetic receipt-policy replacement. It covers raw, enveloped and
metadata-bearing values, literal MessagePack/header-shaped data, zero, early and
late refusal, one-byte-short, exact, maximum and unlimited bounds. Cancellation
is delivered to the sender and checked as a whole terminal outcome with reclaimed
storage. Replayed data cannot publish more bytes. Decompression fixtures supply
worker output; they do not themselves execute a compression codec.

An external whole-open test checks one fresh 16-byte entropy request, one encrypted
cancellation, exact settlement, both peers' reclamation, and no entropy or effects
from replaying the retired worker reservation.

The mixed Tokio/Embassy BLE capstone now runs zero-budget refusal, refusal after
one provisional chunk and successful exact 1,200-byte completion, in both request
directions, followed by buffered request reuse on those links. The complete
simulator suite passed, including all 52 mixed-runtime BLE tests. These are
deterministic virtual radios, not physical RF or whole-board timing evidence.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`; canonical firmware builds do not.

- `cargo test --locked -p prns-core --quiet`, root default tests and
  `cargo test --workspace --locked --quiet`.
- `cargo test --locked -p prns-core --features resource-work-offload --lib external_split_refusal --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `cargo check --locked --manifest-path prns-wasm/Cargo.toml --all-targets` and
  the equivalent command for `benchmarks/Cargo.toml`. These are host compilation,
  not browser execution or a performance measurement.
- `python3 validation/run.py run` with suites `virtual-device-simulation`,
  `bluetooth-auto-embassy`, `embedded-miri-quick`, `embedded-isa-thumbv7em`,
  `embedded-isa-riscv32imac`, and `embedded-isa-xtensa-esp32s3`: all passed.
  Existing Miri/ISA scenarios are regression evidence, not direct execution of
  this new cancellation matrix on those targets.
- Registered `interop-large-request` and `interop-resource-rejection` passed
  with stock RNS in both directions. Their initial sandbox attempts failed with
  `Operation not permitted`; rerunning with loopback access succeeded. These
  suites do not claim exhaustive split-budget interoperability.
- `./tools/prns build embedded resources report --target t114` passed. Static
  RAM remains 142,996 bytes, RAM headroom 360 bytes and estimated stack remaining
  10,352 bytes. Image size is 623,780 bytes (+1,464), flash headroom 142,172 bytes.
  Existing stack-analysis limitations remain; no guard or capacity was reduced.

Generated-contract checking still reports the pre-existing stale
`tools/release/flasher_memory_contracts.py`, left untouched. The readiness doctor
reports insufficient disk space for the 24 GiB matrix requirement and missing
Renode. Full firmware matrix, platform pilots, WASM browser suites, physical
hardware and fresh performance measurements were not run.

Focused manual mutation audit removed the cancellation call while preserving
receiver cleanup and settlement. The mutant compiled, then failed
`cargo test --locked -p prns-core --lib split_response_value_limits --quiet`
because no refusal frame reached the sender. The edit was restored. This is
targeted diagnostic evidence, not an exhaustive mutation score.

## Remaining scope

The simulator's exact segmented budget is 1,200 bytes, not yet the full 2 KiB
Embassy completion boundary. Inconsistent-peer injections and arbitration between
overlapping whole and split responses also remain separate work. No completion
here makes provisional journal chunks independently publishable before success.

# Whole Resource response-size accounting

Local macOS arm64 evidence, 2026-09-25, uncommitted candidate based on `f34e8afe5`.
This is protocol/runtime behavior evidence, not firmware footprint or hardware
qualification. The preceding committed checkpoint passed the normal pre-push
gates, including the sixteen-board resource matrix, embedded Miri and target-ISA
checks, integration capstones and Swift binding smoke; those results do not
qualify this subsequent candidate.

## Correction and independent regressions

Whole, metadata-free Resource responses now count the same encoded value as
packet responses. Admission discounts only the possible fixed outer envelope;
conclusion checks the actual body, including legacy raw values with no envelope
and decompressed responses. Existing transfer/storage ceilings remain intact.
Binary value headers count in full, zero is a real limit, and an enveloped empty
response counts as the one-byte nil value. No wire encoding, shipping buffer,
queue capacity or retained state field changed.

Five of six new owner tests failed against the preceding implementation. They
reproduced exact-limit refusal, a request receipt stranded by a mismatched
enclosed request ID, and delivery despite a false advertised stream length.
The corrected paths return typed failures, deliver no refused body, retire
receipt/transfer state and suppress replay settlement. Metadata-bearing and
segmented admission retains its previous stricter stream bound.

Both mixed-runtime BLE scenarios also failed before the correction. They now
accept exactly 1,200 encoded response bytes, refuse one byte below that budget,
and fill Embassy's existing 2 KiB completion capacity with an exact 2 KiB value.
Subsequent requests reuse the same links. ESP32/Apple and nRF52/BlueZ here name
shared protocol endpoints, not native radio-driver or RF evidence. Existing
queue sizes, time/poll budgets and complete 4,096-event trace bounds are unchanged.

The compression owner test uses known bzip2 fixtures and resumes the core with
their plaintext, testing post-inflation admission and settlement, not the codec
worker itself. The simulation scenarios deliberately use uncompressed data.

## Verification

Host Cargo commands used `CARGO_INCREMENTAL=0` for disk use. Passed:

- `cargo test --locked -p prns-core whole_response_limits --lib` (six tests).
- `cargo test --locked -p prns-core routing::links::resources --lib` (240 tests).
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host whole_response_limits --lib`
  (the six tests also pass without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble`
  (all twelve tests).
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- Root and N-API `cargo fmt --check`, `./tools/prns verify`, and
  `python3 validation/run.py verify`; the parent Rust repository guard also passed.
- `bash validation/hygiene/fmt-docs.sh` (all registered format roots and Rust doc links).
- Registered suites `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `interop-large-request`, and `interop-hopspot-remote-path`, through
  `python3 validation/run.py run --suite …`.
- In `prns-napi`: `node tools/napi-build.mjs --platform --no-js -- --locked`
  (regenerated declarations) and `node --test tests/dist/requests.test.js`.

The focused cargo-mutants audit selected changed lines in `receive/gate.rs` and
`receive/conclude.rs`, using `--in-diff` with their zero-context Git diff. It ran
in place with `--no-config -p prns-core --no-shuffle --timeout 120 --build-timeout 180`,
locked Cargo inputs and the `--lib routing::links::resources` test filter.
All nine viable mutations were caught; two attempted `Default` replacements
were unviable, with no misses or timeouts. `mutation-shard-check` passed.
Artifacts remain in `validation-artifacts/whole-resource-response-limits-mutation/`.

No full PR lane, firmware/resource build, Miri/ISA, other host platform, native
BLE or hardware checks were rerun for this candidate. Metadata and segmented
response accounting remain a [separate follow-up](../../../prns-core/plans/response-size-accounting.md).

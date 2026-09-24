# Metadata-bearing Resource response limits

Local macOS arm64 evidence, 2026-09-25, uncommitted candidate based on `282b331b0`.
This is shared protocol/runtime evidence, not firmware or hardware qualification.

## Correction and boundaries

Whole metadata-bearing responses now count literal file bytes, excluding the
verified metadata block and its length prefix. The advertisement carries no
metadata length, so it cannot establish a nonzero minimum file size. Admission
therefore retains the independent uncompressed-stream, transfer-capacity and
heap-memory limits, with the response-value check after verification/inflation.
No shipping buffer, queue capacity, retained state field or wire encoding changes.

Files are not response envelopes. The shared receiver previously stripped or
rejected files whose leading bytes resembled `[request_id, data]`; it now preserves
all file bytes, including the first segment of a split file. The pinned local
RNS 1.4.2 and 1.5.0 `Link.py` implementations likewise pass metadata-bearing
responses directly to `handle_response`, without envelope unpacking. Segmented
response-size admission remains stricter and is a separate follow-up.

Seven initial owner regressions failed against the preceding implementation.
Nine owner tests now cover arbitrary file/metadata bytes, empty metadata,
zero/exact/overflow/unlimited limits, compression, literal envelope-shaped files,
both split-file segments, malformed metadata and independent storage ceilings.
They check complete response bytes, typed results, retired state and replay
settlement. The compression test resumes the core with a known bzip2 fixture's
plaintext; it is not codec-worker evidence.

Two mixed Tokio/Embassy BLE scenarios exercise the shared static-file command
through both real command lanes. A 1,200-byte file fits its exact budget and fails
at 1,199 without delivery. An envelope-shaped prefix survives intact; Embassy
receives a full 2 KiB file in its existing 2 KiB completion buffer. Subsequent
requests reuse the links. Proofs acknowledge transport completion even when the
requester refuses the final value size. Queue/time/poll bounds and the complete
4,096-event trace are unchanged. The host-only fixture enables the existing
`large-static-responses` feature. Tokio's streaming-file convenience API and its
blocking compression workers are outside this controlled-executor scenario.
ESP32/Apple and nRF52/BlueZ identify protocol endpoints, not native radio evidence.

## Verification

Host Cargo checks used `CARGO_INCREMENTAL=0` for disk use. Completed:

- `cargo test --locked -p prns-core routing::links::resources --lib` (249 tests).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble`
  (14 tests).
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo test --locked -p prns-core --no-default-features --features std,std-sync-host metadata_response_limits --lib`
  (all nine owner tests without resource-work offloading).
- `cargo check --locked -p prns-core --no-default-features`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features prns-simulation/controlled-time --all-targets -- -D warnings`.
- Registered suites `virtual-device-simulation`, `bluetooth-auto-embassy`,
  `interop-large-request`, `interop-resource-rejection`, and
  `interop-hopspot-remote-path`, via `python3 validation/run.py run --suite …`.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, the parent Rust repository guard, and
  `cargo test --locked --manifest-path docs/website/Cargo.toml`.
- In `prns-napi`: `node tools/napi-build.mjs --platform --no-js -- --locked`
  regenerated declarations; `node --test tests/dist/requests.test.js` passed with
  loopback permission after the sandbox denied socket binding.

The focused cargo-mutants audit used the zero-context diff of `receive/gate.rs`
and `receive/conclude.rs` with `--in-place --no-config -p prns-core --in-diff`,
`--no-shuffle --timeout 120 --build-timeout 180`, locked inputs and the
`--lib routing::links::resources` filter. Of twelve mutations, ten were caught,
one attempted `Default` replacement was unviable (the outcome has no `Default`),
and one exposed a test gap: parsing a later file segment as an envelope went
undetected. The split-file regression now places envelope-shaped bytes in both
segments, independently protecting the literal continuation contract. Re-running
that mutation with `--re 'replace == with != in deliver_split_segment'` caught it;
the focused recheck passed `mutation-shard-check`, with no misses or timeouts.
No production change was needed for that finding. Both runs are retained under
`validation-artifacts/metadata-resource-response-limits-mutation{,-recheck}/`.

No full PR lane, firmware/resource build, Miri/ISA, other host platform, native
Bluetooth, hardware or many-node scale checks were rerun for this candidate.

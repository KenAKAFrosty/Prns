# Mixed-runtime segmented Resource responses

Local macOS arm64 evidence, 2026-09-26, candidate based on `563d45b6d`.

## Scenario and detection scope

This slice adds end-to-end simulation coverage, not a new production correction.
The initial two segmented scenarios passed against the current production code.
It follows the [response-size investigation](../../../prns-core/plans/response-size-accounting.md)
without claiming that earlier inconsistent-peer reproductions ran in this simulator.

Both real nodes use a test-only storage layout composed from existing production
tables. A single 512-byte incoming/outgoing Resource slot and a single
incoming/outgoing assembly row force a 1,200-byte static file into multiple
segments. All unrelated tables retain the existing host `GrowableHeap` layout.
The firmware profiles, protocol logic and runtime completion implementations are
unchanged. The existing literal file fixture starts with an unrelated
response-envelope-shaped prefix, so exact byte comparisons also protect file
content from accidental interpretation as protocol framing.

Each ESP32/Apple and nRF52/BlueZ endpoint pairing performs these operations in
both directions over real request handlers, command lanes and virtual BLE:

- A raw zero-budget request settles exactly once as `ResponseTooLarge`, with no
  whole-response or response-segment journal. The observed Embassy responder
  receives the exact `RejectedByPeer` result.
- A raw unlimited-budget request produces exactly three ordered segments with
  the expected link, one consistent request ID, exact concatenated file bytes,
  and one terminal response settlement whose RTT equals elapsed controlled time.
- Two further buffered requests per direction return the full file and measured
  RTT through the real runtime's awaiter/completion API. The single transfer slot
  is reused after refusal and success, without replacing the link.
- No unexpected link closures or plain deliveries occur. Raw response traces are
  empty after the requests, both radios detach exactly once, every BLE connection
  closes, and the medium trace reports no discarded events.

The observer is confined to the integration-test binary. It stores at most eight
events with at most 2 KiB per body and wakes via `Notify`. Raw commands deliberately
leave their journals application-visible; ordinary request futures consume theirs
inside the production runtime. The observer does not supply results to those
futures or implement segmentation. Existing one-millisecond stepping, ten-second
operation deadlines, 256 polls per tick, 128 settlement polls, eight actor slots,
four queued GATT fragments and 20-byte characteristic values remain unchanged.

## Verification

Host Cargo commands used `CARGO_INCREMENTAL=0`. Passed:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble segmented`
  (two scenarios; ten further consecutive runs also passed).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble`
  (19 tests, including whole-response observation and fail-closed body/event bounds).
- Registered `virtual-device-simulation` and `bluetooth-auto-embassy` suites via
  `python3 validation/run.py run --suite …`.
- `cargo test --locked` and `cargo test --workspace --locked`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and
  `cargo run --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- Final root formatting, whitespace and touched-document relative-link checks.

No configured production mutation owner changed. No new mutation, stock-peer
interoperability, firmware/resource, Miri/ISA, native-platform or hardware runs
are claimed for this test-and-documentation slice. Existing separate readiness
diagnostics are not resolved by these passing host checks.

## Coverage limits

No mid-transfer delay, duplication, cancellation, dishonest peer, compression or
reboot faults are injected by these two scenarios. The raw success requests use
unlimited admission, and buffered requests have enough budget for the currently
stricter advertised stream size. Exact cumulative response-value limits remain
open; this does not close the original segmented-accounting follow-up.

The clock and actor schedule are controlled, but Tokio still uses production OS
entropy and boot origins; no byte-for-byte replay or fixed latency claim is made.
Endpoint labels exercise shared BLE protocol paths, not native HCI/Trouble,
CoreBluetooth/BlueZ APIs, RF, or board firmware. The host fixture retains its
existing finite leaked allocations for static runtime APIs; it is not a
many-node memory/throughput benchmark or firmware footprint result.

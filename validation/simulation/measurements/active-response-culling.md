# Active response culling

Local macOS arm64 candidate based on `75106da53`, 2026-09-26.

## Scope and production behavior

The preceding [receipt-pressure slice](receipt-pressure-response-recovery.md)
fixed assembly retention when a newer request displaced a receipt between
segments. This slice tests displacement during an admitted continuation, followed
by delivery of its late data. Existing shared-core cleanup passes these cases;
there are no production, wire, capacity or timing-policy changes.

The private simulator fixture retains one receipt, one incoming/outgoing Resource
slot, one assembly slot and a 512-byte transfer window. Eight pressure scenarios
cover raw calibration and ordinary buffered APIs in both Tokio/Embassy directions
for virtual ESP32/Apple and nRF52/BlueZ endpoint pairings. Two no-pressure controls
exercise both directions with the same hold/release operation and require the
complete file and exact measured RTT.

## Fault and recovery contract

1. Hold the third outgoing `Resource` data-frame send attempt for the response
   link, after the continuation advertisement has been admitted. The raw journal
   calibrates this fixture-specific cut: exactly the first provisional segment
   has arrived, with its exact file prefix, link, request ID, index and count.
   The gate counts matching wire headers; it is not a generic segment decoder.
2. Issue a real buffered Echo request on the same link to displace the old receipt.
   Wait for the old raw or buffered request to report `Culled`, then release the
   held part. The replacement must still return its exact body and measured RTT.
   Buffered file requests must return the typed failure, not a partial success.
3. Advance 120 seconds on the coordinated virtual clock while keepalives continue.
   Active-transfer retries have a longer watchdog than advertisement retries;
   the twenty-second drain used by the preceding slice is insufficient here.
   The Embassy responder journals Echo success followed by Resource timeout.
   This does not establish immediate sender-slot reclamation or change its policy.
4. Complete a new three-segment file on a different existing link to prove the
   receiver's sole assembly/transfer slots are reusable. Then exercise refusal,
   raw success and repeated buffered success on the original links. BLE remains
   connected, with no link-closure journal or node reset.

Pressure scenarios allow 130 seconds of virtual time with 256 actor polls per
tick and a bounded 524,288-event radio trace; overflow is checked by fixture
cleanup. The larger trace is test-only. These cases do not prove zeroization,
all cancellation/transfer phases, cumulative value limits, whole/split-response
arbitration, physical-radio behavior or replay with controlled production entropy.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`. Passed:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble active_culling --quiet`:
  ten tests, including both no-pressure controls; five final-code repeats passed
  all fifty test executions.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy`:
  both registered suites passed, including all 52 mixed-runtime BLE tests and
  55 Embassy BLE owner tests.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and `git diff --check`.

No new firmware builds, Miri/ISA runs, hardware tests or performance measurements
are claimed for this test-only slice. Endpoint names identify virtual backend
contracts, not execution on those operating systems or physical boards.

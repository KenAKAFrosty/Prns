# Buffered response failure through a wire-gated BLE outage

Local macOS arm64 evidence, 2026-09-26, candidate based on `84e8719ae`.

## Scenario and detection scope

This slice closes a coverage gap in the
[raw-command interruption scenarios](interrupted-segmented-resource-simulation.md):
ordinary request futures consume response journals internally. Instead of
changing that production routing, a private test backend wrapper holds an
outgoing frame before handing it to the existing virtual GATT sink. Encryption,
segmentation, framing, request completion and all runtime policies remain real.
No production defect or correction is claimed by this slice.

The gate matches an entire `WirePacketHeader` for the selected link and pauses
the second Resource advertisement send attempt. It has one armed/held slot per
radio, a nonzero occurrence count and notification-driven waits. It retains no
payload copies or observation queue. Releasing forwards the original bytes to
the underlying sink; cancelling the held send releases gate ownership. Other
backend/link methods delegate unchanged. This wrapper exists only in the
`embassy_ble` integration binary, not the public simulator library or firmware.

For both ESP32/Apple and nRF52/BlueZ endpoint pairings, and both request directions:

- Raw-command calibration requires exactly one verified response segment with
  index one of three, the expected link, one observed request ID and the exact
  literal file prefix, followed by exactly one `SendRequestFailure::Timeout`.
- The same wire cut during an ordinary buffered request must return exactly
  `SendError::Failed(SendRequestFailure::Timeout)`, not a successful prefix.
  No response body or request settlement may leak onto the application journal.
- The cut itself must close the one BLE connection, and the released/cancelled
  gate must be idle when the request finishes.
- The Embassy responder must report exactly
  `RespondFailure::Resource(SendResourceFailure::Timeout)` by request completion,
  with no second settlement after reconnection. This differs from the earlier
  cut immediately after the response journal: here the first proof already
  advanced the sender to the next advertisement, whose unanswered retry budget
  expires before the Reticulum link retires.
- Both old links must report `LinkClosedReason::Timeout` on both nodes. Fresh
  links then rerun zero-budget refusal, raw three-segment success and repeated
  full buffered success using the same nodes and single-slot Resource storage.
- Two small Embassy requests must complete concurrently after recovery. A
  compile-time assertion ties this probe to both available request-completion
  slots; sequential success alone could mask a leaked slot. Distinct whole
  response values and measured RTTs must match, excluding stale response bytes.

A separate no-outage control holds and releases the same advertisement in both
endpoint pairings, then verifies the exact full file, measured RTT, successful
responder settlement, unchanged connection and subsequent requests. Gate unit
tests cover header/occurrence selection, disarmed/malformed/unrelated pass-through,
overwrite refusal, cancellation/rearming, and wakeups without advancing paused
time. The usual bounded traces, operation budgets and final radio cleanup apply.

## Verification

Host Cargo commands used `CARGO_INCREMENTAL=0`. Passed:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble wire_gate --quiet`
  (four gate tests).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble buffered_interruption --quiet`
  (five tests, including raw calibration and no-outage control; ten consecutive
  final-candidate repeats also passed, fifty further test executions).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble`
  (32 tests).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation --suite bluetooth-auto-embassy`.
- `bash validation/hygiene/fmt-docs.sh`, `./tools/prns verify`,
  `python3 validation/run.py verify`, and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- Root formatting, whitespace and touched-document relative-link checks.

## Coverage limits

Header occurrences count send attempts, not decrypted segment ordinals. The raw
calibration proves this particular honest-peer cut follows one verified segment;
the gate is not a generic segment decoder. Buffered internals are not inspected:
the assertion is the public failure result and clean subsequent use, not memory
zeroization. No corrupted/compressed input, dishonest peer, application-request
cancellation, reboot or native radio fault is injected.

Recovery still waits for old-link retirement, so live-link assembly cleanup and
cumulative value accounting remain in the
[response-size investigation](../../../prns-core/plans/response-size-accounting.md).
Production Tokio entropy and boot origins are not controlled; this is not a
byte-for-byte replay claim or a many-node memory/throughput measurement. No
shipping core, runtime, storage profile, wire format or RAM budget changes. No
new mutation, stock-peer interop, firmware/resource, Miri/ISA, native-platform
or hardware evidence is claimed.

# Competing packets while a continuation is being received

macOS arm64, based on `46d5592fd`, 2026-09-26.

## Scope and observation

This extends the mixed-runtime [competing-packet scenario](competing-packet-response.md)
from the gap between Resource segments into active continuation reception. There
is no production change: the existing shared-core ownership guard already covers
this phase.

The scenario forwards the first segment's two wire parts, then drops subsequent
Resource data parts on that link. Advertisements and Resource requests still
flow. After observing the first verified segment, it waits for an actual dropped
part before issuing the competing packet through the real responder API. The
receiver has requested continuation data; the test does not merely assume that
admission occurred after an arbitrary delay.

The requester must publish no competing value or premature settlement. While
continuation data remains blocked, another link completes its own whole response.
Restoring data delivery then yields exactly the original three segments and one
successful settlement, with exact bytes and RTT. Existing recovery checks reuse
the same links for refusal, raw and buffered response cases. Both request
directions run for ESP32/CoreBluetooth and nRF52/BlueZ profiles over virtual BLE.

The wire gate gains a metadata-only first-loss observation, using its existing
notification and counter. It neither copies frames nor queues observations. A
paused-time unit test covers waiting before loss, observing after loss, unrelated
frames, allowed occurrences, and rearming without retaining an old observation.
Existing between-segment scenarios also use the observation to establish their
loss point explicitly.

## Diagnostic evidence and limits

Temporarily disabling the shared packet-response split-ownership guard made the
new focused scenario fail at the no-extra-response assertion. Restoring it passes.
No diagnostic mutation remains. This is additional coverage of an existing fix,
not a newly discovered production defect.

The scenario still observes raw requester journals, not competing values injected
through buffered request APIs. It is not an independent whole-Resource peer,
worker-result replay, physical Bluetooth stack test, or exhaustive overlap proof.

## Verification

Passed on macOS arm64; tests and clippy used `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble first_loss_is_observable --quiet`.
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble competing_responses --quiet` (three scenarios).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet` (60 tests, including gate unit tests).
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo fmt --all -- --check`, `bash validation/hygiene/fmt-docs.sh`, and `git diff --check`.

Firmware, Miri/ISA, physical hardware, stock interoperability and benchmarks were
not rerun for this test-only slice.

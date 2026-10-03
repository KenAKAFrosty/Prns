# Retired response identities cannot complete replacement buffered requests

macOS arm64, based on `839e1bb22`, 2026-09-27.

## Scenario matrix

This extends the [buffered competition matrix](buffered-response-competition.md)
across request retirement and subsequent reuse. A normal buffered echo request
first ends by either success or timeout. The responder-side metadata probe retains
its request ID. Timeout cases drop exactly one real echo response and require the
buffered API to report `Timeout` at precisely the configured 1,000 ms deadline.

A new buffered file request then uses the same link. At one of three observed
loss points (before its first verified segment, between segments, or during
continuation reception), the responder issues a fresh authenticated packet naming
the retired request ID. The probe verifies that the replacement has a different
request ID. This is a late response for an ended request, not a byte-for-byte
duplicate that packet-hash deduplication could discard on its own.

An independent buffered echo completes on the same link while the file remains
blocked. The file future must still be pending. After loss ends, it must return
the exact 1,200 file bytes and measured RTT, with no orphan response event or
duplicate request settlement leaking to the application. Embassy responder
settlements account for the original echo and all subsequent responses.

Four tests independently cover success/timeout for Tokio/Embassy requesters.
Each runs all three file-reception phases under ESP32/CoreBluetooth and
nRF52/BlueZ profiles: 24 combinations. Both Embassy request slots are reused
concurrently afterward, and the existing same-link refusal/raw/buffered recovery
checks run in both directions. No node or link is reset to obtain recovery.

The shared phase/loss/injection helper now lives in `segmented/competition.rs`,
used by both the current-claim and retired-claim scenarios. Its typed claim
distinguishes current ownership from a specific retired request ID. Observation
remains bounded; there are no new payload queues or shipping APIs.

## Diagnostic evidence

Temporarily broadening the shared receipt lookup to match the authenticated link
but ignore request ID made all four tests fail at the pending-file assertion.
The late packet incorrectly settled the replacement request. Each diagnostic
stops on its first phase/profile combination; this is not a claim of a full
24-case mutation campaign. Restoring exact identity matching passes the entire
matrix. No mutation remains in production code.

This adds assurance for existing behavior, not a new production fix. It does not
exercise request-future cancellation, request-ID collisions, independent
whole-Resource peers, worker replay, or physical radio/OS stacks. Original request
retirement uses packet echo replies; the replacement response is segmented.
Firmware layout, retained production state and wire formats are unchanged.

## Verification

Passed on macOS arm64, with `CARGO_INCREMENTAL=0` for tests and clippy:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble retired_response --quiet` (four tests, six combinations each).
- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble --quiet` (74 tests, including observer/gate unit tests).
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware matrix, Miri/ISA, physical hardware, stock interoperability or
benchmark rerun is claimed for this test-only slice.

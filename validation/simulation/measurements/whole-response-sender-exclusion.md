# Whole-response sender exclusion and independent-link progress

Local macOS arm64 candidate based on `f724dcf8b`, 2026-09-26.

## Finding and scope

This slice adds simulator assurance only. No production behavior, firmware
capacity, retained state or wire format changes.

Attempting to reproduce whole-Resource competition through the normal responder
API revealed an existing safeguard: the sender refuses the second Resource on
the active link as `SendResourceFailure::Rejected(LinkBusy)`. The initial test
expected a receiver rejection and failed on that distinction. It did not reproduce
a whole offer reaching the receiver or establish receiver-side safety.

The test now asserts the actual contract rather than attributing sender exclusion
to receiver arbitration. Deliberately inconsistent peers, pre-admitted whole
transfers, queued promotion and delayed completion still require receiver-focused
coverage before the whole/split overlap gap can be considered closed.

## Deterministic scenario

Both real runtimes take the requester and responder roles for ESP32/CoreBluetooth
and nRF52/BlueZ compatibility pairings over virtual BLE. After the first verified
segment, continuation advertisements are dropped while other traffic remains
live. The responder attempts a competing packet or small whole-file response.
The packet remains covered by the preceding shared-core ownership fix; the whole
Resource is refused locally.

A separate, independently established link then completes a 128-byte whole-file
response, with exact bytes and one settlement, while the original split request
is still pending. Restoring advertisements completes all three original chunks
and exactly one successful settlement. The existing recovery sequence reuses the
original links afterward.

The embedded responder journal explicitly distinguishes packet-command success
from whole-Resource `LinkBusy`, followed by success for the independent request
and original split response. The Tokio fixture does not collect generic responder
settlements; that direction checks requester effects, independent progress and
recovery rather than claiming direct observation of its sender error.

The overlap fixture explicitly supplies two transfer slots and two outgoing
assembly slots, retaining one incoming split-assembly slot. Ordinary scenarios
keep their single-slot profile. This separates per-link exclusion from global
storage exhaustion without changing any shipping storage profile. A second
properly established requester-side link is used; reversing an existing link's
requester/responder roles is not part of this fixture's contract.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble competing_responses --quiet`: both focused tests passed.
- `python3 validation/run.py run --suite virtual-device-simulation`: all passed,
  including 56 mixed-runtime BLE tests.
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `bash validation/hygiene/fmt-docs.sh` and
  `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.

Firmware builds, Miri/ISA, physical hardware, interoperability and benchmarks were
not rerun for this test-only slice. Virtual endpoint profiles do not execute the
native Bluetooth stacks or establish physical radio/timing behavior.

# Retiring the middle of three pending callers

macOS arm64, based on `63820f84b`, 2026-09-27.

## Review slice 1

Three exchanges are interrupted across rapid BLE reconnects, with explicit
seven-millisecond gaps and strictly increasing start times. After all three
recoveries, but before the first deadline, the middle caller is cancelled.

Four tests cover uniform request/response loss and queued/consumed fragment
boundaries. Both request directions, both CoreBluetooth/BlueZ profile assignments
and cyclic plus version-one seeds 0, 7 and u64::MAX yield 64 configurations,
each running two batches of three losses on the same link.

Cancellation must return `Cancelled` once and `NotLive` on repetition. It must
not move the clock or change BLE data observations. The registered actor count
falls from six to five. The first caller still times out at its own original
deadline, the middle deadline produces no completion, and the third caller
times out at its original deadline. Exact completion maps and actor counts
are checked at all three boundaries, with fresh concurrent traffic between them.

The original keep-all tests remain controls. Route/ingress checks, unchanged
node IDs, distinct payloads, coordinated clocks, bounded traces and complete
detach remain active. No actor or transport capacity is enlarged.

Production impact: none. This changes tests only; it does not establish private
runtime completion-table reclamation or raw application-journal behavior.
Profiles are virtual GATT, not native Bluetooth. No firmware, hardware,
Miri/ISA, benchmark or exhaustive-scheduling result is claimed.

## Verification commands

Host Cargo commands use `CARGO_INCREMENTAL=0`:

The focused four tests and all twelve flapping tests passed, as did clippy,
root/workspace tests and the registered simulation suite (46.687 seconds).
Existing ignored tests remain ignored.
Repository formatting/docs checks and `git diff --check` passed.

```console
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet cancelling_middle --quiet
cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet flapping --quiet
cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings
cargo test --locked --quiet
cargo test --workspace --locked --quiet
python3 validation/run.py run --suite virtual-device-simulation
```

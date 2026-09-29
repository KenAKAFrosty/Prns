# Blocked sends and stale-link wake schedules

Local macOS arm64 candidate based on `8b46e308a`, 2026-09-26.

## Discovery and scope

The existing wire gate holds an outgoing frame inside the real BLE send future;
it does not retain a detached copy after that future is cancelled. Trying to release
a continuation advertisement after request timeout showed that the runtime had
already cancelled the blocked send. The scenario was corrected to test that real
behavior rather than claim late-frame injection.

Continuing on the old links then exposed a deterministic debug assertion: the
cached receipt-timeout schedule remained armed at 24,510 ms while a full engine
recompute reported Idle. The call came from the Embassy link-deadline timer path.
Shared-core `fire_due_link_deadlines` can retire stale links, which removes receipt,
channel and Remote Control pairing state, but returned no updates for those three
wake families. It now returns their current schedules alongside link and Resource
deadlines. This benefits every runtime using the API; no new retained state or wire
format is introduced. Only receipt drift is dynamically reproduced here; the
channel and pairing updates follow the same teardown's inspected ownership.

## Exact scenario

The manually driven mixed-runtime BLE test runs ESP32/Apple and nRF52/BlueZ
profiles. An Embassy responder's second segmented-response advertisement is held
inside its send future. The Tokio requester must observe exactly the first verified
segment and one Timeout, with no further response events. The held-send guard must
have been released by cancellation, without a manual gate release. The sender's
response settles as a Resource timeout.

A new Embassy request on the other existing link remains pending during stale-link
closure and must fail as LinkClosed, not Timeout. Both nodes must report the exact
two timed-out old links. Fresh links must have different identities; the existing
reassembly checks then verify capacity refusals and successful segmented and
buffered responses in both directions. Final fixture checks enforce empty journals,
idle gates and radio cleanup.

This uses real runtime tasks, deadlines and simulated BLE, not physical radios.
There is no independently authenticated conflicting-peer injection yet, and this
slice does not add it or exercise worker-pool scheduling.

## Verification

Host Cargo uses `CARGO_INCREMENTAL=0`. Removing the three wake updates makes the
finalized test fail at the same receipt-schedule assertion; restoring them passes.
No diagnostic mutation remains. Passed:

- `cargo test --locked -p prns-simulation --features controlled-time --test embassy_ble blocked_continuation_sends --quiet`.
- `cargo test --locked -p prns-core --lib engine::deadlines --quiet` (39 tests).
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `cargo clippy --locked -p prns-core -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo check --locked -p prns-core --no-default-features`.
- Registered `virtual-device-simulation`, now including 57 mixed-runtime BLE tests.
- Registered `embedded-miri-quick` (three existing assurance scenarios, not this
  new mixed-runtime scenario).
- `bash validation/hygiene/fmt-docs.sh` and `cargo fmt --all -- --check`.
- `cargo run --quiet --locked --manifest-path ../contributing/repo-guards/Cargo.toml --target-dir target/repo-guards`.
- `git diff --check`.

The final test-only removal of an unused helper return was rechecked with the
focused scenario and simulation clippy. No firmware size/build matrix, target-ISA,
hardware, stock interoperability or benchmark rerun is claimed. Free disk was
approximately 464 MiB; no comparative firmware or performance claim is made.

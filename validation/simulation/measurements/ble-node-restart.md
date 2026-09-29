# BLE node replacement and old-connection isolation

macOS arm64, based on `1916ac31e`, 2026-09-27.

## Full-node coverage

Two new `manual_fleet` scenarios use four real Tokio nodes and BLE supervisors
in two isolated pairs. One restarts the CoreBluetooth-profile node and the other
the BlueZ-profile node. Each performs three teardown/rebuild cycles using the
selective actor cancellation API, not radio disable/enable. The backend and
engine are rebuilt with the same Bluetooth address, BLE identity and configured
Reticulum destination identity.

The scenario has four live radio slots, one neighbor/connection/discovery entry
per backend, one supervisor peer slot, eight actor slots, two-entry control and
fragment queues, and a 262,144-event trace. GATT data values are 20 bytes; exact
256-byte request/response payloads therefore exercise real fragmentation and
reassembly. Existing inline crypto keeps the node work inside the manual runner.

Each cycle checks that:

- Cancelling the selected node closes only its pair's BLE connection, removes
  that member from the surviving peer's inventory and leaves three node actors.
- The unrelated pair continues exchanging on its original Reticulum link.
- Replacement uses fresh task/radio IDs while preserving the supervisor's
  logical interface identity. Discovery and announcement propagation converge
  within the existing 60-second virtual-time budget.
- The peer inventory contains exactly the expected connected identity, not an
  old/new duplicate. Re-cancelling the old task returns `NotLive`.
- Disabling a retained old status handle cannot disable the replacement
  supervisor or close its new connection.
- A new Reticulum link has a different link ID. Requests on the obsolete link
  remain pending through 99 milliseconds and time out exactly at 100, while
  the fresh and unrelated links both return their own complete payloads.

Orderly final shutdown leaves no actors or connections. All seven radio
attachments detach exactly once, with no dropped observations or truncated
trace. The unaffected nodes are never restarted and the simulation clock is
never reset during these cycles.

## Backend boundaries

Four focused backend tests add seven cases beneath the real-node scenarios:

1. A deliberately queued sighting from the old radio cannot authorize a dial to
   a physically reachable replacement at the same address. The discovery
   snapshot remains bound to the old radio until a fresh advertisement is
   consumed. Only then may the new connection open.
2. A queued control PDU is not delivered after restart, and a second control send
   blocked behind it wakes with `LinkClosed`.
3. A queued complete frame cannot be returned through its old source or retargeted
   to the replacement; the caller's output canary remains unchanged.
4. A one-fragment queue forces a larger send to block while the receiver consumes
   an incomplete prefix. Restart wakes both operations with `LinkClosed`; no
   partial frame is published and a replacement connection exchanges exact new
   frames in both directions.

The last three cases run with either the dialer or listener backend replaced.
They retain old links or data halves across the new connection's creation, then
drop them and prove the new connection still works. Each uses two live radio
slots and verifies three attachments/detachments without trace loss.

These backend-only tests use a paused Tokio test runtime with a one-second
watchdog and explicit medium steps. They do not claim full production-node
scheduling coverage; that is supplied separately by the manual-fleet scenarios.

## Diagnostic and limits

An initial helper advanced across two advertisements and mistakenly assumed
the next sighting must be fresh after replacement. The existing incarnation
guard correctly refused that dial. The normal helper now steps one medium event
at a time; the separately named queued-sighting test preserves that boundary
explicitly rather than assuming address equality implies a current observation.

Temporarily removing the existing observed-radio equality guard made that test
fail with `Started` instead of `UnknownPeer`. The guard was restored before final
verification. This is focused diagnostic evidence, not a new shipping-code fix
or an exhaustive mutation campaign.

Production behavior, wire formats and firmware memory budgets are unchanged.
CoreBluetooth/BlueZ are protocol profiles over virtual GATT, not execution of
the native macOS/Linux Bluetooth stacks. These new full-node cases use Tokio,
not Embassy firmware. Destructors run during teardown; physical power loss,
controller/DMA reset, flash recovery, persisted identity provisioning, pooled
workers and byte-for-byte entropy replay remain outside this slice.

## Verification

Passed on macOS arm64; host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --lib backend_restart --quiet` (four tests).
- `cargo test --locked -p prns-simulation --features controlled-time --lib --quiet` (104 tests).
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet` (seven tests).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

No firmware, Miri/ISA, native Bluetooth, hardware smoke or benchmark run is
claimed for this test-only slice.

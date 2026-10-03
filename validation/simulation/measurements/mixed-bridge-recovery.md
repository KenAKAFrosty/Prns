# BLE interruption versus mixed-bridge restart

macOS arm64, based on `3af85ba3b`, 2026-09-27.

## Scope

Four manual-fleet tests extend the three-node frame/BLE bridge. Each runs with
either CoreBluetooth or BlueZ as the bridge's BLE profile and the other profile
at the BLE-only endpoint. Each profile case performs two interruption/recovery
cycles. These are protocol profiles over virtual GATT, not native OS stacks.

The frame-only endpoint, dual-interface transport and BLE-only endpoint are real
Tokio production nodes using inline crypto. The medium bounds are two frame
endpoints, one neighbor each, eight queued frames per endpoint, eight pending
frame deliveries, two radios, one BLE peer per supervisor and eight actors.
Trace capacities are 32,768 frame events and 262,144 BLE events. GATT data values
are 20 bytes and complete echo values are 256 bytes.

## Radio-only interruption

Three tests respectively isolate BLE reachability, disable the bridge's BLE
supervisor, and disable the BLE endpoint's supervisor. The disable/enable cases
use the existing local status control API; they are not Remote Control protocol
tests and do not change desired-state command behavior.

Before disruption, both endpoints have actual two-hop routes through the bridge,
with the expected ingress interface IDs. Crossing links exchange in both
directions; a separate frame-only link connects the frame endpoint to the
bridge's own destination.

Every cycle verifies:

- Interruption settles without advancing the clock or replacing a node actor.
  The BLE connection count becomes zero and both supervisor member inventories
  become empty.
- Repeating the same disabled/isolated state produces no further BLE trace
  change. The original frame-local link remains usable throughout the outage.
- Crossing requests remain pending through millisecond 49 and time out exactly
  at 50. Both media and the runtime retain a coordinated timeline while BLE
  advertisement events and request timers are driven.
- Reconnection restores exactly one expected member per supervisor, not duplicate
  old/new entries. Explicit announcements and production route inspection check
  the usable two-hop paths again.
- Existing logical Reticulum links resume with exact new payloads. Fresh links
  can also be established and used; they have distinct IDs. Both old and fresh
  links are retained for the next cycle, where they all encounter the outage.
- Repeating `enable` on both already-enabled supervisors changes no BLE trace
  and does not interrupt traffic. All node task IDs and clock origins remain
  unchanged across both cycles.

An initial test incorrectly expected the old logical links to remain unusable
after BLE reconnection. Real requests succeeded instead. This was a mistaken
test assumption, not a production defect: the transport and endpoint engines
still retained their logical link state. The corrected test now explicitly
requires that recovery and uses different payloads to exclude an old response
masquerading as completion of a new request.

## Bridge process restart

The fourth test cancels the bridge actor itself, removing both its frame interface
and BLE supervisor. The two endpoint actors remain live. Requests on the old
frame-local link and both crossing links time out at exactly 50 milliseconds.
The BLE peer loses its member, and the common clock does not move merely because
an actor is destroyed.

The bridge is rebuilt with the same configured identity and interface tags,
then explicitly reconnected to both media. Its task ID is fresh. Retrying
cancellation of the old task yields `NotLive`. Fresh local and crossing links
must differ from their predecessors and carry the expected complete payloads.

The old links cannot resume in this case: requests on all three time out exactly
while new links exchange successfully. A retained status handle from the retired
BLE supervisor is disabled after replacement; it must not close the replacement
connection. Both original endpoint task IDs remain unchanged.

Per-node clock observations prove that surviving nodes retain elapsed time from
their original boot, while each rebuilt bridge has a new boot origin on the same
global timeline. The process is repeated without resetting the clock.

## Cleanup and interpretation

All cases end with orderly node shutdown, no actors, no pending frame deliveries
and no active BLE connections. Every frame endpoint and radio attachment detaches
exactly once: two per medium for radio-only cases, four per medium for the two
bridge-restart cycles. Neither trace may truncate, no frame receive queue may
overflow, and no BLE discovery observation may be dropped.

Historical announce inventories can remain populated across radio reconnection;
the tests do not pretend those callbacks alone establish fresh discovery. Actual
route inspection and successful traffic provide the path-recovery evidence.
Reconnection and rediscovery are explicitly driven by the harness, not evidence
of automatic application retries. Recovery of logical links after a short radio
outage does not promise recovery after long enough for protocol link expiry.

## Verification

Host Cargo commands use `CARGO_INCREMENTAL=0`:

- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet mixed::recovery --quiet` (four tests; two profile assignments and two cycles each).
- `cargo test --locked -p prns-simulation --features controlled-time --test manual_fleet --quiet` (twenty-one tests).
- `cargo clippy --locked -p prns-simulation --features controlled-time --all-targets -- -D warnings`.
- `cargo test --locked --quiet` and `cargo test --workspace --locked --quiet`.
- `python3 validation/run.py run --suite virtual-device-simulation`.
- `bash validation/hygiene/fmt-docs.sh` and `git diff --check`.

Shipping behavior, wire formats and firmware memory budgets are unchanged. No
native Bluetooth, firmware, Miri/ISA, hardware smoke, physical power-loss,
persistence or benchmark result is claimed. OS entropy in production crypto
and announcement jitter still prevents byte-for-byte full-node replay.

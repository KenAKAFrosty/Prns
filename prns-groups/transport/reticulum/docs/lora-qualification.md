# Checking an isolated LoRa path

The intended trial path is phone A → Bluetooth → board A → LoRa → board B →
Bluetooth → phone B. A device receipt alone does not prove that path. Nearby
phones/boards can form shorter Bluetooth routes, and Wi-Fi, ESP-NOW, TCP or USB
may provide alternatives.

## Available controls and preconditions

Bluetooth discovery groups provide a reversible peer-selection partition. Both
handshake roles reject peers with no group in common before creating a Reticulum
data interface. Replacing groups tears down affected peer lanes and in-flight
handshakes. These names are selection hints, not secret credentials or group
membership authorization.

A management client can submit the canonical
`RemoteControlRequest::ReplaceInterfaceDiscoveryGroups` with the actual interface
ID. Board persistence and runtime application require readback after restart.
The embedding must explicitly configure and restore both local phone sets, or
use a separately verified physical isolation setup. Board controls alone do not
isolate the phones.

Before changing settings, establish authenticated management access to each board,
compare the required pairing confirmation, and retain an independent way to
restore access. Read the loaded firmware's supported capabilities and actual
interface IDs. Preserve startup faults and logs; do not erase settings or regenerate
identities to make a trial appear healthy.

## Reversible procedure

1. Record each board's interface inventory, discovery groups, radio profile and
   power state, plus each phone's discovery configuration and enabled interfaces.
   Bind observations to authenticated device identities and exact app/firmware
   builds. Preserve message stores and capture the settings needed for restoration.
2. Where local discovery configuration is available, move one pair at a time:
   phone `{original, trial-a}` → board `{trial-a}` → phone `{trial-a}`; use
   `trial-b` for the other side. Read back the board after reconnection before
   removing the overlap. The final sets must have no cross-side name in common.
   A lost reply during intentional disconnection requires readback, not blind retry.
3. Disable unused board and phone network paths. Keep only the intended Bluetooth
   and LoRa lanes. Check interface power after asynchronous changes and every
   reboot. Verify matching LoRa frequency, bandwidth, spreading factor, coding
   rate and framing. Exclude third-party bridges and development TCP connections.
4. Confirm each phone sees only its intended board and the boards have no cross-side
   Bluetooth lane. Preserve application data if runtimes must restart to clear old
   Links/routes. Record route, Link and interface-byte counters before sending.
5. Send a unique message once and wait for its intended device receipt. Correlate
   both boards' LoRa TX/RX byte changes and the phones' selected routes over the
   complete interval. Interface bytes include enrollment, announcements and retries;
   they are not per-record airtime or frame counts. Repeat in the opposite direction.
6. Disable LoRa on one board and read back its state while keeping both phone–board
   Bluetooth links available. Submit another unique message once. Record the
   observation window and actual attempt evidence. A delivery reveals an alternate
   path; silence without an attempted exchange is inconclusive. Restore LoRa and
   require that same message and receipt to converge without another send.
7. With LoRa restored, repeat interrupted-contact recovery by disabling the
   receiving phone's Bluetooth and then restoring it. Reopen a phone with its
   existing store and verify retained messages/receipts without duplicate sends.
8. Restore every captured radio profile, interface power state and discovery set.
   Use temporary discovery overlap to retain management access. Read back the
   restored state and identities; reboot alone does not restore persisted radio
   configuration. Record mismatches instead of applying remembered defaults.

## Observation and timeout limits

The library supplies opt-in metadata diagnostics through
`PRNS_GROUP_DIAGNOSTICS=1` or `enable_metadata_diagnostics()`. The embedding must
install a logger for `prns.groups.diagnostics` and `prns.groups.delivery`.
Producers have fixed event budgets and cap markers; a quiet capped or missing
sink is not evidence of inactivity. Group/peer/record tags use a process-local
salt and cannot be compared across restarts or devices. No content or raw keys
are emitted.

A push-start event is an attempted exchange, not proof of RF transmission: path
resolution or Link establishment may still fail. A successful exchange carrying
an accepted MLS receipt establishes durable record acceptance through that
exchange. Correlated isolation/counter evidence is still needed to attribute the
path to LoRa. Local-send events are observations, not an exhaustive commit log;
a cancelled waiter can omit one and an exact retry can repeat it.

Use the actual consumer's diagnostic capabilities and report missing coverage.
A receipt alone does not establish continuous diagnostic coverage.

Link deadlines include route/interface allowances, while the group scheduler
also applies contact spacing, contention and failure backoff. No fixed short
wait proves that a selected record was attempted. Choose the negative-control
window from the active configuration and retain actual attempts. A timed-out
waiter does not abandon a host-owned establishment or its late-success cleanup.

Host tests use real protocol objects and controlled carriers. They do not qualify
RF range, interference, regulatory airtime, Bluetooth coexistence or physical
power-loss recovery. Keep source/binary hashes, configuration readbacks, timestamps,
logs, diagnostic caps and restoration results with each device trial.

## Source owners

- [Bluetooth handshake](../../../../prns-core/src/interfaces/bluetooth_auto/handshake.rs): discovery-set matching.
- [Tokio Bluetooth runtime](../../../../prns-interfaces/impls/tokio/src/bluetooth_auto/runtime.rs): replacement and peer teardown.
- [Board management](../../../../personal-hopspot/embedded/esp32/src/s3/remote_control.rs): persistence, rollback and power operations.
- [Scheduling policy](../../../core/src/sync.rs) and group runner: bounded network work.

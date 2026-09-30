# Bluetooth session recovery

This is the implementation sequence for native Bluetooth session recovery. The
ownership repairs, retained controls and correlated Android/embedded control-write
completion below are implemented. Negotiated liveness remains planned. There is
no new control-wire version or automatic restart-recovery claim in this change.

A controlled two-phone iOS restart check delivered a baseline message, restarted
only iOS, and then failed recipient resolution while Android retained an
established member and rejected new
candidates. Resetting only Android's app Bluetooth restored delivery of the same
unsent draft. This motivates shared transport work, not contact retries or an
application-owned Bluetooth protocol.

## Ownership and current repairs

- The pure connection policy in `prns-core` keeps the healthy incumbent. A BLE
  greeting asserts a transport identity; it does not authenticate an LXMF sender.
  A competing Hello alone must not evict a working connection.
- Tokio member close notifications carry the exact admission's
  `TokioInterfaceStatus` identity, compared using `same_instance`. A delayed
  notification cannot remove a newer member, even at the same identity and
  address, or trigger backend cleanup for it.
- Android Rust links retain one physical-connection lease through handshake,
  both data halves and the settled native control owner. Its final drop retires
  only the owned connection and checks the queue identity against connection-ID
  reuse. Address-level policy cleanup no longer closes every physical link at
  that address.
- Android platform I/O requires its own callback ownership and deadlines; a
  Rust queue admission is not a completed GATT write. These bounds cannot detect
  an idle dead application whose operating-system connection still appears live.

The SDK owns generic Android platform mechanics; the same transport helpers are
consumed by Hopspot. The application consumes public host APIs and owns contacts,
mail, names and product policy. LXMF authenticated-key retention uses the
existing core retention API independently of Bluetooth recovery.

## Retain controls through settlement

The shared `BleLink::into_parts` seam returns source, sink and an optional
transport-only `BleControl` owner. Native connections retain the control owner
for their entire session, including legacy connections. Columba
has no native control channel and returns none. Handshake operations forward to
the same control implementation; no second codec or policy belongs in backends.

Tokio, Embassy, Apple, Android, BlueZ, WinRT, Trouble, nRF and their test adapters
use this split. Columba's data-only path never creates and immediately drops a
control owner. Embedded slot leases broadcast a latched closure on **any** owner
drop, while the last owner releases slot reuse. The native worker and all
control/data owners retain that slot until exit. BlueZ requires the native data
characteristic rather than assigning its control stream two competing readers.

Apple's peripheral tracks an explicit handshaking, settled or retired phase for
each exact session. Dropping the retained control owner retires that session;
a closed handshake receiver alone does not authorize replacing a settled peer.
Atomic batch admission, rollback and ownership of pending GATT/L2CAP work are
preserved. A fresh Hello cannot evict a healthy incumbent. A failed handshake
can be replaced only after both receivers end. An unprobeable legacy incumbent
cannot be declared dead solely from a competing Hello.

Tokio owns one persistent control-loop future per member alongside its data
work, including a blocked outbound send. Embassy retains the control owner in
its live member slot and polls cancellation-safe receives alongside idle and
duplex data work. A control Close or receive error ends only that member. Late
Hello/Welcome messages are ignored with bounded yielding so they cannot starve
data or timers. Columba continues without a native control loop.

The supervisor still only receives after settlement. It does not introduce
probes, shutdown messages or concurrent control writes. The completion work
below supplies persistent pending-operation state and control/data write
arbitration for the queued Android and embedded backends; the future liveness
loop must keep each possibly submitted write alive across unrelated data work.

### Retained control validation

Focused validation passes 27 Tokio Bluetooth tests, 63 Embassy/Trouble tests,
65 simulation BLE unit tests, 102 controlled-time Embassy integration tests and
135 FFI host tests. One FFI test still requires real radio hardware and is ignored.
Strict Clippy passes for those host implementations. The checks cover blocked
sends, cancellation, ignored greetings, closed control channels, exact-session
cleanup, Apple batch rollback and embedded slot reuse.

iOS, Windows, Linux and t-echo SoftDevice v6 target checks compile. Windows and
Linux tests were not executed; the Linux cross-check used host D-Bus metadata and
does not establish target linking. Firmware flash/RAM budgets and rebuilt-phone
restart behavior are not qualified by these checks.

## Bound actual control I/O

Android and embedded queued controls now carry an operation sequence through
submission and completion within the exact session owner. Queue admission alone
does not finish a control send. The backend determines its completion point:

| Backend | Local completion point |
| --- | --- |
| Android client | Matching characteristic-write callback |
| Android server | Matching notification callback, fenced by registration and retained owner |
| nRF central | Acknowledged GATT write response |
| nRF peripheral | Connection-local notification completion count from `on_notify_tx_complete` |
| Trouble central | Acknowledged GATT write response |
| Trouble peripheral | Subscribed notification admitted to the host's outbound queue |

None of these completion points proves peer liveness or remote application
receipt. Trouble checks the exact connection's subscription rather than accepting
the notification API's unsubscribed no-op as success. Its single-executor server
cannot process a subscription change between that check and packet assembly.

Each queued backend retains the original message identity, sequence, deadline
and terminal result outside cancelable send futures. Resuming the same send
joins its existing operation, including while waiting for capacity; another
message cannot consume its result. Timely completion remains available after a
delayed resumption, but a late callback cannot turn expiry into success.
Embedded receipts hash the canonical control bytes only to check resumption
consistency, not to authenticate the peer. This resumption contract is specific
to these implementations; callers must not assume every `BleControl` backend
can resume a canceled send.

Acknowledged control and data writes share one lane per physical connection.
For nRF and Trouble, canceling or failing an admitted write retires that
connection before releasing its untagged response lane. Canceling a waiter that
has not acquired the lane does not poison the active operation. Independent
receive and L2CAP work continue. nRF registers each notification before native
submission; surplus completion counts never become credit for a future write.
Its existing four shared control-wire buffers remain bounded and are held until
the worker completes. Pool availability uses bounded broadcast waiters, not a
single consumable wakeup; this is an admission hint, not a fairness guarantee.

Android's shared Rust bridge and private SDK/Hopspot JNI adapters carry exact
operation receipts. Both Kotlin consumers use the native codec's control
capacity, including a maximum 153-byte greeting, and distinguish an undersized
buffer from no output. No public host contract or generated application binding
changes are needed. Callback registration precedes native submission, and a
submitted-ticket watermark rejects a stale pump read after completion. Rust
watchdog scheduling is bounded and does not keep a physical connection leased.

Submission, Busy retries and completion share the original deadline. Expiry
retires the physical attempt, not merely its operation lane. Android retains
uncertain server notification/disconnect ownership and fences registration
changes. Platform callbacks do not themselves carry the Rust operation ticket;
receipt matching therefore cannot distinguish arbitrary duplicate OS callbacks
for successive writes on the same characteristic and physical client. Exact
native ownership, serialization and timeout quarantine remain necessary.

Address quarantine is safe but is not automatic recovery. A future automatic
server-registration reset must explicitly retire all affected peripheral
sessions; it cannot pretend only one peer was reset. Keep that policy separate
from completion handling.

### Control completion validation

The focused checks pass 31 Tokio Bluetooth tests, 90 Embassy/Trouble tests,
65 simulation BLE unit tests and 102 controlled-time Embassy integration tests.
FFI host tests pass 145 tests with one existing hardware-radio test ignored;
the SDK JNI adapter passes two host tests. Strict all-target Clippy passes for
Tokio, Embassy, FFI and the SDK JNI adapter.

Both JNI consumers compile for Android ARM64. Production Kotlin compilation and
unit tests pass for both consumers: 63 SDK tests and 33 Hopspot tests. The SDK
check uses its exact source through the existing aggregate app consumer; it
does not build or install a new phone binary. Repository validation/tooling
registries, formatting and documentation-link checks also pass.

Linked resource checks pass for t-echo with both S140 versions and for E290,
without reducing features, connection limits, pool capacity or reservations.
The nRF control and L2CAP pools now use zero-initialized claim flags, placing
their unchanged four and nine buffers in `.bss` instead of copying zero-filled
storage from flash at startup. Outlining canonical control decoding and pinning
native operations in caller-owned storage also avoid duplicated async code and
storage.

| t-echo profile | Firmware bytes | Flash headroom | RAM headroom after reservations |
| --- | ---: | ---: | ---: |
| S140 v6 | 620,748 | 5,940 | 1,180 |
| S140 v7 | 620,860 | 1,732 | 1,180 |

Both retain the 69,632-byte runtime-stack reservation. The v7 margin remains
small and must be checked again as liveness is added. These linked-size checks
do not measure peak runtime memory use or qualify physical Android callbacks,
nRF notification completion, or rebuilt-phone restart recovery. No phone or
board has been installed with this slice.

## Negotiate liveness without breaking legacy greetings

Keep Hello/Welcome and their discovery-group encoding unchanged. There are no
unused capability bits in their PSM/MTU fields. Do not encode features in RSSI,
fake discovery groups, or an unrecognized greeting suffix.

Use a new optional read-only characteristic in the native GATT service, with a
strict versioned capability value. A central discovers and reads it on the
current physical connection. Absence or an unsupported value retains legacy
behavior; no cached fact from a previous connection enables enforcement. An
accepted optional read that never completes must terminate its attempt rather
than letting a subsequent GATT operation overlap it. Embedded generated clients
must not make the new characteristic mandatory when discovering old services.

Only a central that read a supported value may initiate a session probe. The
listener learns that its client supports the extension from a valid probe on
that session, not from advertising or merely serving the characteristic read.
A correlated reply establishes responsiveness; unrelated messages and local
send completion do not. Legacy peers and Columba never acquire a new timeout
because they cannot answer a probe.

Put bounded probe state, nonce matching and deadline decisions in a small
`no_std` core policy with caller-provided monotonic time. Tokio and Embassy
provide timers and retire the exact session. Handle duplicate challengers with
bounded/coalesced ownership while checking an incumbent; do not turn repeated
untrusted greetings into unbounded work or automatic eviction. Define and test
the probe cadence, response deadline and suspension behavior before enabling
the capability on any backend.

## Shutdown and acceptance

For cooperative app Off, stop admitting work, request a bounded supported close
while native pumps still run, then drop the session owners and stop the radio.
Radio loss or permission loss still requires immediate local teardown. Do not
send a new shutdown reason to legacy peers or label normal shutdown as a
duplicate-link rejection.

Automated gates must cover both old/new directions, unsupported and stalled
capability reads, healthy keepers, silent incumbents, duplicate or late probe
replies, concurrent challengers, cancellation during send, stale callbacks after
replacement, and graceful-close timeout. Include embedded slot reuse and Apple
batch rollback regressions. Run backend target builds, including generated
embedded GATT discovery, before advertising support.

Physical acceptance repeats the controlled baseline/restart/send journey in
both directions on rebuilt iOS and Android binaries, without manual announce,
Bluetooth reset, or an alternate transport. Record delivery, duplicate count,
unchanged persisted records and actual reconnect time. App Off/On, platform
radio loss, permission denial, long idle and background suspension remain
separate acceptance cases. Passing queue or codec tests does not qualify them.

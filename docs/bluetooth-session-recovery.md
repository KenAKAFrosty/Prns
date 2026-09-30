# Bluetooth session recovery

This is the implementation sequence for native Bluetooth session recovery. The
ownership repairs, retained controls and correlated Android/embedded control-write
completion below are implemented. Negotiated liveness is implemented for the
Tokio runtime with Apple and Android backends. Rebuilt-phone restart acceptance
is still required; implementation and host tests are not an automatic-recovery
claim. Embedded and other backends retain legacy behavior.

A controlled two-phone iOS restart check delivered a baseline message, restarted
only iOS, and then failed recipient resolution while Android retained an
established member and rejected new
candidates. Resetting only Android's app Bluetooth restored delivery of the same
unsent draft. This motivates shared transport work, not contact retries or an
application-owned Bluetooth protocol.

The [September 29 mobile checkpoint](../applications/checkpoints/2026-09-29-mobile-persistence-recovery.md)
repeats that failure on the reconciled Release builds after separately fixing and
verifying interval persistence. Reciprocal baseline delivery and an isolated
Android restart send pass. The iOS restart remains blocked; the diagnostic peer
reset is not automatic-recovery acceptance.

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

Embassy still only receives after settlement. Tokio additionally drives the
negotiated probe policy described below. It retains each started send across
unrelated data work; controls and GATT data share a physical write lane in the
mobile backends. Neither runtime introduces a new shutdown message in this slice.

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

## Negotiated mobile liveness

Hello/Welcome, their discovery groups and the 153-byte control limit are unchanged.
The native service's optional read-only `NATIVE_LIVENESS_UUID` characteristic
uses suffix `e9` and exactly six capability bytes, `50 52 4e 53 01 01`. Unknown values,
missing characteristics and completed read errors preserve legacy behavior.
The read happens before notification subscriptions, within the original native
startup deadline. An accepted read that never completes ends that physical
attempt; it cannot release its lane for a subsequent operation.

Tokio opts in before starting the radio. Backends default to disabled, so a
consumer that does not drive the protocol cannot advertise working support.
Android snapshots capability publication per server registration. Apple's
prepared service can exist before the runtime is ready; its dynamic read rejects
requests until opt-in and rejects requests from an already admitted legacy
session. A restored service without the characteristic remains legacy until
normal publication. Restoration never reuses prior central capability evidence.

Only a central that read supported bytes on this exact connection initiates
probes. Its first probe is immediate after settlement. A listener answers and
starts its own checks only after receiving a valid probe on that session, not
after merely publishing the characteristic. The fixed session mode is never
reconstructed from an address. Columba has no native control channel and remains
outside this protocol. Embedded, BlueZ and WinRT do not opt in in this slice.

The allocation-free core policy owns at most one outstanding probe, one pending
reply and one active write. Probe and reply wire tags are `04` and `05`, each
followed by an eight-byte big-endian nonce. Nonces start from fresh session
entropy and increment without wrapping; they correlate responses but do not
authenticate a peer. The timings are:

- Initiators probe immediately; listeners first probe 30 seconds after activation.
- A matched reply schedules the next probe 30 seconds later.
- Each probe has 30 seconds from its original due time for both native write
  completion and the matching reply. Queue waits and retries do not renew it.
- A reply has one 30-second write budget, capped by any earlier outstanding
  expiration. Excess incoming probes are ignored, with at most one reply
  admitted per second and no replacement of the retained nonce.

Wrong, late or duplicate replies, greetings, data activity and competing
connections cannot postpone these deadlines. Tokio checks expiration before
queued input and retains one write future until it settles or the session ends.
Long local suspension may therefore expire a connection on resume; that means
responsiveness was not established within the budget, not that the remote app
died. A listener whose peer disappears before sending its first valid probe is
still unprobeable and retains legacy behavior.

Healthy incumbents remain protected. Challengers are rejected through the
existing bounded connection policy; they neither trigger eviction nor create a
pending challenger queue. After an incumbent expires, normal discovery and retry
can admit a replacement. No app retry or periodic Bluetooth reset is added.

The core Bluetooth suite passes 107 tests, including strict decoding, unchanged
greetings, nonce/deadline rules and bounded policy storage. All 42 Tokio Bluetooth
tests pass, including two negotiated peers, passive legacy listeners, blocked
writes, data activity and dropping exact member owners before reporting expiry.
Core/Tokio strict Clippy and the core no-default-features check pass. The full core
library passes 2,105 tests with three existing ignores. The 90 Embassy/Trouble,
65 simulation BLE and 102 controlled-time integration tests still pass without
enabling embedded probes.

FFI passes 164 tests with one existing hardware-only ignore, strict Clippy and
the iOS target check. Android's shared helpers pass 75 SDK and 45 Hopspot Kotlin
tests; both private JNI consumers pass host tests, strict Clippy and Android
ARM64 target checks. Both Android consumers preserve their existing eight-second
startup limit; Apple retains its original 15-second limit. Late callbacks cannot
extend startup by beating a delayed watchdog. Firmware resource and physical
phone checks remain separate gates.

## Shutdown and acceptance

Cooperative shutdown notification remains follow-up work: stop admitting work,
request a bounded supported close
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
First confirm that both nodes remain Running through an interval save; a stopped
persistence worker must not be misdiagnosed as Bluetooth recovery failure.

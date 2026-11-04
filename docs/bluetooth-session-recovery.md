# Bluetooth session recovery

Native Bluetooth recovery combines session ownership, retained controls,
correlated control-write completion and optional negotiated liveness. Tokio
drives negotiated liveness with Apple and Android backends; embedded and other
backends retain legacy behavior. The contracts below describe the implementation.
Physical restart, silent-peer expiry, long idle and OS lifecycle behavior need
separate acceptance on the actual app and firmware builds.

## Session ownership

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
  reuse. Address-level policy cleanup does not close every physical link at
  that address.
- Android platform I/O requires its own callback ownership and deadlines; a
  Rust queue admission is not a completed GATT write. These bounds cannot detect
  an idle dead application whose operating-system connection still appears live.

The SDK owns generic Android platform mechanics; the same transport helpers are
consumed by Hopspot. The application consumes public host APIs and owns its address book,
conversation presentation and product policy. LXMF authenticated-key retention uses the
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
mobile backends. Neither runtime introduces a new shutdown message.

## Bound actual control I/O

Android and embedded queued controls carry an operation sequence through
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
buffer from no output. These receipts belong to the private transport bridge. Callback registration precedes native submission, and a
submitted-ticket watermark rejects a stale pump read after completion. Rust
watchdog scheduling is bounded and does not keep a physical connection leased.

Submission, Busy retries and completion share the original deadline. Expiry
retires the physical attempt, not merely its operation lane. Android retains
uncertain server notification/disconnect ownership and fences registration
changes. Platform callbacks do not themselves carry the Rust operation ticket;
receipt matching therefore cannot distinguish arbitrary duplicate OS callbacks
for successive writes on the same characteristic and physical client. Exact
native ownership, serialization and timeout quarantine remain necessary.

Address quarantine does not automatically reset server registration or recover
all affected peripheral sessions.

## Negotiated mobile liveness

The native control protocol uses Hello/Welcome, discovery groups and a 153-byte
control limit.
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
session. Restoration never reuses prior central capability evidence.

Apple replaces a known legacy restored service at startup, after PoweredOn and
before listener admission. It removes only that exact service and publishes a
fresh one; a complete current service retains its existing objects. Live owners
defer replacement, and malformed or duplicate restored Prns services fail startup
without removal. Readiness accepts only the matching publication callback.
Reads, writes and subscription callbacks also match current characteristic
objects, so delayed same-UUID callbacks cannot affect the replacement. Mixed
old/new write batches still fail atomically.

Only a central that read supported bytes on this exact connection initiates
probes. Its first probe is immediate after settlement. A listener answers and
starts its own checks only after receiving a valid probe on that session, not
after merely publishing the characteristic. The fixed session mode is never
reconstructed from an address. Columba has no native control channel and remains
outside this protocol. Embedded, BlueZ and WinRT do not opt in.

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

Android consumers retain an eight-second startup limit; Apple retains a
15-second limit. Late callbacks cannot extend startup by beating a delayed
watchdog. Resource gates and physical phone acceptance remain separate from
codec and ownership tests.

## Limits and validation

Competing connections, startup retries and long-idle stability require physical
qualification before making recovery-time guarantees. A greeting's claimed
identity is unauthenticated; it does not authenticate an address association.
The connection policy does not implement identity-based alias suppression.

Shutdown tears down local session owners and radio resources. It does not send
a negotiated graceful-close notification to settled peers. Radio or permission
loss also requires local teardown. Tests of incoming control Close messages do
not establish an outgoing shutdown protocol.

The automated suites exercise supported and legacy peers, unsupported and
stalled capability reads, incumbent protection, probe deadlines, cancellation,
and callback ownership. The owning interface and platform guides describe
backend-specific checks. Firmware builds and physical phone acceptance cover
properties that these controlled tests cannot establish.

Physical acceptance repeats the controlled baseline/restart/send journey in
both directions on rebuilt iOS and Android binaries, without manual announce,
Bluetooth reset, or an alternate transport. Record delivery, duplicate count,
unchanged persisted records and actual reconnect time. Repeat the lifecycle cases
in both native roles. Current-build Android app-interface off/on, iOS system-radio
recovery, permission loss, silent-peer expiry, long idle and natural suspension
remain separate acceptance gaps. Passing queue or codec tests does not qualify them.
First confirm that both nodes remain Running through an interval save; a stopped
persistence worker must not be misdiagnosed as Bluetooth recovery failure.

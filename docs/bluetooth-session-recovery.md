# Bluetooth session recovery

This is the implementation sequence for native Bluetooth session recovery. The
ownership repairs, retained controls and correlated Android/embedded control-write
completion below are implemented. Negotiated liveness is implemented for the
Tokio runtime with Apple and Android backends. September 30 BLE-only trials on
rebuilt phones passed isolated app restarts in both directions and subsequent
verified saved-contact delivery. Later bounded lifecycle trials passed node and
radio recovery and off-screen receipt. Silent-peer expiry, long idle and natural
OS suspension remain unqualified. Embedded and other backends retain legacy behavior.

A September 29 two-phone iOS restart check delivered a baseline message, restarted
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

### Historical control completion validation

Before negotiated liveness, the completion-only slice passed 31 Tokio Bluetooth
tests, 90 Embassy/Trouble tests, 65 simulation BLE unit tests and 102 controlled-time
Embassy integration tests.
FFI host tests passed 145 tests with one existing hardware-radio test ignored;
the SDK JNI adapter passed two host tests. Strict all-target Clippy passed for
Tokio, Embassy, FFI and the SDK JNI adapter.

Both JNI consumers compiled for Android ARM64. Production Kotlin compilation and
unit tests passed for both consumers: 63 SDK tests and 33 Hopspot tests. The SDK
check used its exact source through the existing aggregate app consumer; it
did not build or install a new phone binary. Repository validation/tooling
registries, formatting and documentation-link checks also passed.

Linked resource checks passed for t-echo with both S140 versions and for E290,
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

Both retained the 69,632-byte runtime-stack reservation. These are historical
completion-only measurements; the liveness measurements below supersede them.
These linked-size checks do not measure peak runtime memory use or qualify
physical Android callbacks, nRF notification completion, or rebuilt-phone restart
recovery. No phone or
board was installed as part of these checks.

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

FFI passes 174 tests with one existing hardware-only ignore, strict Clippy and
the iOS target check, including the restored-service upgrade regressions. Tests
and iOS checks also pass with logging enabled. Android's shared helpers pass
75 SDK and 45 Hopspot Kotlin tests; both private JNI consumers pass host tests,
strict Clippy and Android ARM64 target checks. Both Android consumers preserve
their existing eight-second startup limit; Apple retains its original 15-second
limit. Late callbacks cannot
extend startup by beating a delayed watchdog. Linked resource checks and physical
phone acceptance remain separate gates.

### Liveness resource validation

The configured resource gates pass for both t-echo profiles and E290, with all
five canonical memory contracts unchanged and ordinary linker overflow checks
enabled. These figures include the liveness codec but do not enable embedded
probes. No features, connection limits, pool capacity or reservations were reduced.

| Target | Firmware bytes | Flash headroom | Internal RAM headroom after reservations |
| --- | ---: | ---: | ---: |
| t-echo S140 v7 | 621,436 | 1,156 | 1,180 |
| t-echo S140 v6 | 621,324 | 5,364 | 1,180 |
| Heltec E290 | 2,347,392 | 12,779,136 | 42,188 |

Both t-echo profiles retain the 69,632-byte runtime-stack reservation. The v7
flash margin remains small and requires rechecking after code changes.

Adding the probe variants exposed a compiler code-size regression in control
decoding: fusing the parser's `Result` with the public decoder's `Option`
conversion expanded aggregate greeting copies even on the short probe path.
Keeping `Control::try_decode` out of line reduced v7 `.text` by 1,400 bytes
against the overflowing liveness build, without changing parsing, wire bytes or
static RAM. The single `#[inline(never)]` boundary restored fit; the 107 core
Bluetooth tests, strict all-target Clippy and no-default-features check passed
again with this final source.

The three JSON receipts are retained under
`/Volumes/wavlink/dev/prns-mobile-build/control-firmware/resources/configured/reports/`:
`t-echo-s140-v7.json`, `t-echo-s140-v6.json` and `heltec-e290.json`. Matching linker
maps and stack evidence are under the sibling `work/<target>/` directories.
Each receipt records the same frozen working-tree source:

```text
kind: working-tree
head: 472d5fb07750f2e08fed27e5bc9bc6ab1d1a7e4b
diff_fingerprint: 4612f9fd864370141455deac262da5c3a5f91a578ad56494257f662589e477d6
```

That source was subsequently committed as
`8d641e6dea4560729e8ddebfd01c492a6b2aa178`. These are working-tree receipts, not
clean-commit receipts for that commit or later revisions. The later Apple-only
capability-read ownership fix, `87c7350d6`, is outside the firmware targets and
is not part of these receipts. They establish linked fit under the recorded
configuration, not peak runtime memory use or physical recovery. The later
Android diagnostic and Apple restored-service changes are also outside these
firmware targets and receipts.

### September 30 phone acceptance

Both phones were updated in place, without clearing identities, contacts or
messages. Initial BLE-only delivery passed both ways, but restarting only Android
left a subscribed connection without a working route. Diagnostics then showed
the optional liveness characteristic absent from Android's service view. Legacy
delivery subsequently worked; the missing capability alone does not establish
the full cause of that failed trial.

After the startup-only Apple restored-service upgrade repair, Android observed
the capability without a radio reset or Android restart. Each app was then
restarted separately while the other phone's process remained unchanged.
Replacement links negotiated liveness and exchanged recurring controls. Android
reconnected about 22 seconds after its restart command; iOS reconnected about
18 seconds after its command. The first attempted saved-contact sends, tested
about 100 seconds after each restart, delivered with matching message IDs and
receiver Verified source. No manual announce, Bluetooth reset or alternate
transport was used. Both phones retained their records and completed interval
saves; loaded conversation counts matched the eight new logical messages.

These are bounded restart observations, not a recovery-time guarantee or proof
of silent-peer timeout expiry: Close controls were observed during recovery.
Before the controlled restarts, an unexplained control-write error and connection
churn also recovered automatically; its cause remains open. Long-idle stability,
natural suspension and permission loss remain separate cases.
The app checkpoint records artifact hashes, timestamps and the earlier failure.

### Lifecycle continuation

The same installed binaries then passed Android whole-node Stop/Start, iOS
app-level Bluetooth off/on, and Android system Bluetooth off/on. Their first
attempted saved-contact sends after recovery delivered with matching IDs and
receiver Verified source. Neither app process changed, although Android radio
recovery recreated its native runtime. Android's system switch was off while
the OS retained a BLE-only state for other services; this was user-visible radio
recovery, not complete hardware power removal.

Both phones also received while off-screen: iOS after more than five minutes,
and Android after seven minutes with its foreground node service active. The
sender reported delivery before the receiving app returned to the foreground;
stored messages and subsequent replies had matching IDs and Verified source.
USB stayed connected and iOS Mirroring remained available, without an attached
debugger. Recurring controls and data continued. These observations do not prove
natural suspension, deep sleep, relaunch or silent-link expiry. Resumed sends
were attempted about 85 seconds after return, not immediately on resume.

The Android off-screen trial included an unexplained native status 8 disconnect
after about 16 minutes connected. A replacement negotiated about seven seconds
later and carried the successful message. An earlier node Stop/Start trial also
needed a replacement after a status 8 disconnect. Rejected challengers preceded
both events, but the logs do not establish causation. Separately, nine competing
candidates were rejected in about a minute while the incumbent continued carrying
traffic. Recovery passed these trials; uninterrupted or churn-free operation did not.

The seven new logical messages appeared once in the bounded loaded histories.
Identities, contacts, other displayed conversation counts and the iPhone's paired
board remained present; both apps completed interval persistence. No data clear,
manual announce or alternate transport was used. These are individual trials,
not recovery-time guarantees or whole-database audits.

## Remaining hardening

Investigate the competing-connection churn and native status 8 disconnects before
claiming long-idle stability. A narrow first candidate is the outbound-handshake
failure path that currently clears its per-address backoff. A bounded cooldown
must not pause unrelated addresses or reject inbound peers. Any policy change
needs focused ownership/retry tests and renewed firmware resource checks.

Do not indefinitely suppress an address because a greeting claims an existing
peer's identity. That identity is unauthenticated; even a responsive incumbent
does not authenticate the claimed address association. Address reuse, rotation
and forged identities must retain bounded recovery opportunities. No alias
suppression or retry-policy change is implemented in this slice.

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
unchanged persisted records and actual reconnect time. Repeat the lifecycle cases
in both native roles. Current-build Android app-interface off/on, iOS system-radio
recovery, permission loss, silent-peer expiry, long idle and natural suspension
remain separate acceptance gaps. Passing queue or codec tests does not qualify them.

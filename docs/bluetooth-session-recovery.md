# Bluetooth session recovery

This is the implementation sequence for native Bluetooth session recovery. The
ownership repairs below are implemented; retained controls and negotiated
liveness remain planned. There is no new control-wire version or liveness claim
in the current build.

A controlled two-phone iOS restart check delivered a baseline message, restarted
only iOS, and then failed recipient
resolution while Android retained an established member and rejected new
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
  notification cannot remove a newer member, even at
  the same identity and address, or trigger backend cleanup for it.
- Android Rust links now retain one physical-connection lease through handshake
  and both data halves. Its final drop retires only the owned connection and
  checks the queue identity against connection-ID reuse. Address-level policy
  cleanup no longer closes every physical link at that address.
- Android platform I/O requires its own callback ownership and deadlines; a
  Rust queue admission is not a completed GATT write. These bounds cannot detect
  an idle dead application whose operating-system connection still appears live.

The SDK owns generic Android platform mechanics; the same transport helpers are
consumed by Hopspot. The application consumes public host APIs and owns contacts,
mail, names and product policy. LXMF authenticated-key retention uses the
existing core retention API independently of Bluetooth recovery.

## Retain controls through settlement

Extend the shared `BleLink` seam with a transport-only control owner and a split
returning source, sink and optional control. Native connections retain the
control owner for their entire session, including legacy connections. Columba
has no native control channel and returns none. Handshake operations forward to
the same control implementation; no second codec or policy belongs in backends.

Change all split callers together: Tokio, Embassy, Apple, Android, BlueZ, WinRT,
Trouble, nRF and their test adapters. Do not implement a legacy data-only wrapper
by creating and immediately dropping the control owner. Embedded slot leases
signal closure on **any** owner drop, while the last owner releases slot reuse.
The native worker and all control/data owners must retain that slot until exit.

Apple's peripheral currently recognizes a fresh Hello after settlement through
the closed handshake receiver. Replace that heuristic with an explicit
exact-session phase when controls stay open. Preserve atomic batch admission,
failed-admission rollback and exact ownership of any retired GATT/L2CAP session.
The phase is only a demultiplexing fact: it must not authorize retiring a healthy
incumbent on a fresh Hello. Keep the incumbent until exact physical-generation
loss or the shared bounded liveness decision permits replacement, and bound or
coalesce pending challengers. An unprobeable legacy incumbent cannot be declared
dead solely from a competing Hello.

Tokio owns one persistent control-loop future per member alongside its data
work. Data activity must not repeatedly cancel and resubmit an in-flight control
write. Embassy retains control and pending-operation state in its live member
slots and schedules it alongside existing data work without allocating a Tokio
task or an unbounded per-peer queue.

## Bound actual control I/O

Before advertising lifecycle support, Android and embedded queued control
writes need correlated native-worker completion receipts. Carry the exact
session owner and an operation sequence through submission and completion.
Success means the platform's completion point, or stack admission where the
platform provides no later completion. It never proves remote receipt.

Use one overall deadline for submission and response; Busy retries do not start
new deadlines. A missing callback retires the physical attempt, not merely its
operation lane. Android server callbacks carry only an address: retain the old
notification/disconnect owner until completion, and fence registration changes.
When completion ownership remains ambiguous, address quarantine is safe but is
not automatic recovery. A future automatic server-registration reset must
explicitly retire all affected peripheral sessions; it cannot pretend only one
peer was reset. Keep that policy separate from the smaller timeout repair.

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
First confirm that both nodes remain Running through an interval save; a stopped
persistence worker must not be misdiagnosed as Bluetooth recovery failure.

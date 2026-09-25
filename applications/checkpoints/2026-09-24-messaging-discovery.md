# Messaging names, discovery and recipient selection

The messaging journey is implemented in source. Local automated checks passed.
On the builds preceding the final Apple notification repair, retained-data
installation, reciprocal discovery and proof-backed delivery passed.
Discovery-clear sending passed on both phones, and Android cold-launch resolution
passed without a remote reannounce. Automatic recovery after iOS cold launch
remains blocked by stale
Bluetooth peer ownership, detailed below. A subsequent Apple notification repair
is built and installed; its final physical message retry is pending device access.
Earlier [Bluetooth control acceptance](2026-09-24-local-bluetooth.md) and
[transport feasibility](2026-09-23-ordinary-corebluetooth.md#physical-continuation-direct-unpaired-ble-messaging)
do not qualify these new binaries. The
[two-phone demo plan](../docs/phone-node-demo.md) remains the broader milestone.

## Implemented behavior

**Contacts** contains the saved messaging name, a collapsed **Edit messaging
name** form, **My address**, and explicit **Announce yourself**. **My address →
Share address** opens the platform share sheet; it does not announce anything
on the network. Saving the name is also separate from announcing. The announce
control is disabled while a name save is pending.

The name defaults to `prns`. Rust trims it, requires a nonempty name without
control characters, and limits its encoded length to 64 UTF-8 bytes. It is
stored through the existing native database owner and can be read or changed
while the node is stopped. Changing it updates the registered LXMF announce
data, including path responses, without replacing the primary identity,
restarting the node or discarding mail. A saved-but-not-applied outcome and an
unconfirmed write have distinct UI feedback; neither is described as a rollback.

**Saved** and **Discovered** are separate views. Discovered contains authenticated
LXMF destinations, deduplicated by destination and excluding this phone. Each
row shows a name, short address, last-heard age, **Save contact** and **Message**.
**Discovery details** exposes the recorded ingress interface, hop count,
identity and whether the observation was a path response. An ingress ID is
matched to an exact physical Bluetooth peer first, then the logical Host
inventory. It is never inferred from a connected-peer count. These details
describe the last received announcement, not a future message route.

Saving a discovered contact rechecks native authenticated metadata and identity
conflicts. The directory stores its announced name separately from its private
alias and derives whether it is a messaging destination. Private aliases take
display priority and are not overwritten by discovery. Existing contact records
without an announced name remain readable. Saved contacts remain available
without an active node generation.
When an authenticated discovery matches a manually saved address that has no
messaging identity, **Save messaging contact** upgrades that record through the
same native checks while preserving its private alias and pin setting.

Discovery has a 256-entry capacity and a 24-hour monotonic age limit within the
native generation. **Clear discovered contacts** hides current observations
without deleting saved contacts, mailbox records, or bounded recipient metadata.
A subsequent accepted announcement can make a contact visible again. Clearing
this view does not block a peer or change routing.

**Inbox → New message** selects a saved messaging contact or discovered
recipient, with **Enter address** as a secondary path. Saved nonmessaging
destinations have no Message action. Inbox derives conversations from stored
messages rather than creating empty conversations for every announcement.

A fresh send uses valid authenticated recipient metadata or requests it through
one bounded native path lookup, including when a host route already exists.
The lookup has a 15-second limit and at most eight concurrent waiters. Identity
and stamp requirements are checked before a durable message is admitted and
rechecked if admission waits. **Finding contact…** does not mean a message is
queued. Lookup and identity failures keep the unsent draft available for an
explicit retry; only native durable acceptance opens the conversation.
Existing proof-backed delivery, stored-record retry and cancellation remain in
place. A late result cannot navigate a departed composer or a reused route
that now targets another recipient.

## Ownership and source

React owns presentation and user intent. The app's native composition owns
profile storage, contact policy, supervisor admission and the one aggregate
host session. The reusable LXMF service owns bounded authenticated observations
and recipient resolution. The shared host exposes accepted-announce provenance
through its native embedding observer; the UI does not consume diagnostic
announces as authenticated identities or create a second event owner.
The canonical HostSnapshot and the general Expo SDK remain separate from app
contact/profile policy. Generated UniFFI values carry the product contract.

- [Profile normalization and address derivation](../prns/native-composition/src/messaging_profile.rs),
  [database owner](../prns/native-composition/src/development_store.rs),
  [contact storage](../prns/native-composition/src/directory.rs), and
  [lifecycle admission](../prns/native-composition/src/lifecycle/admission.rs).
- [Bounded peer metadata and resolution](../services/lxmf/src/direct/peers.rs),
  [durable sends](../services/lxmf/src/mailbox.rs), and
  [shared native embedding](../../prns-host/impls/native/src/embedding.rs).
- [Contacts](../prns/app/src/features/contacts/contacts-screen.tsx),
  [messaging profile UI](../prns/app/src/features/contacts/messaging-profile.tsx),
  [directory reads](../prns/app/src/features/contacts/messaging-directory.ts), and
  [Inbox and composition](../prns/app/src/features/inbox/inbox-screen.native.tsx).

## Local automated evidence

- The final complete app run passed **392 tests in 39 suites**; the focused app and
  navigation run passed **120 tests in seven suites** after
  the final UI fixes. It covers manual-only announcing, honest outcomes,
  offline profile access, private-name precedence, authenticated save commands,
  conflict feedback, discovery clearing, recipient selection, message-backed
  conversations, late results, reused composer routes, and Stop/Start during a
  pending lookup. Narrow-width interaction checks at 1.5x and 2x text cover
  profile editing and discovery details; they are not physical layout evidence.
- The platform facade passed **65 tests in six suites** and its complete code
  verification. Generated API drift, app/SDK aggregate foreign-object sharing,
  compatibility and dependency boundary checks passed.
- App source, test and tool TypeScript checks passed. Full app formatting/lint
  and whitespace checks passed. Route validation retains **33 catalog rows
  and 34 route leaves**, with no new navigation paths.
- The native composition passed **199 library tests** and strict all-target
  Clippy with the Apple/UniFFI features. Coverage includes retained profile
  storage, cancelled waiters, live name updates without identity/generation
  replacement, stopped-node profile reads with an existing identity, directory
  compatibility and identity conflict handling.
- The LXMF service passed **47 unit tests and two real-host integration tests**,
  plus strict all-target Clippy. Its
  [recipient-resolution integration](../services/lxmf/tests/link_packet_layering.rs)
  starts with no recipient cache or route, learns an updated registered name
  from a path response, and reaches durable Delivered. It then restarts the
  service while retaining the host route and resolves again: two explicit path
  requests, two path responses, two Delivered records and zero unsolicited
  remote announces. This is local host evidence, not a Bluetooth device test.

- Final Apple Bluetooth recovery passed **91 transport tests** with one separate
  hardware test ignored, strict all-target Clippy and explicit iOS-target Clippy.
  The regression first reproduced ATT error 17 from a fresh exact-peer Hello
  reaching a closed handshake receiver, then passed with atomic stale-session
  replacement. Failed batch admission preserves the existing session. An old
  inbound L2CAP link also closes when its authoritative GATT session ends.
  Four of these tests cover notification backpressure, missed-ready prevention,
  ordering, bounded capacity, cancellation, timeout and exact-session fencing.
- The Android SDK passed **42 JVM tests in five suites**, including seven new
  write-admission/completion regressions. Both final platform release builds
  succeeded. A Java 25 Android packaging attempt failed in `jlink`; the passing
  build uses Java 21. That toolchain failure is retained in the local logs.

Native builds and physical observations are recorded separately below. Hosted
CI has not run for these local commits.

## Known announce timing limitation

The real-host restart test exposed an existing core replay-policy boundary.
[Announce IDs](../../prns-core/src/routing/announce/id.rs) use whole-second origin
timebases. For an existing route at the same hop count and without a stronger
interface preference, the
[acceptance policy](../../prns-core/src/routing/announce/acceptance.rs) rejects
new evidence whose origin timebase is not newer, even with a different nonce.

In the initial test, restarting the service and requesting another path within
the same origin second produced a second request and response but no accepted
peer observation. Recipient resolution therefore timed out. The passing
integration test explicitly crosses that boundary with a 1.1-second interval
before restarting the service and resolving again. This preserves core replay
semantics; there is no new delay or automatic repeated path request in the app.
A rapid service restart with a retained host route can still require an
explicit send retry after the first bounded lookup fails. This edge has not
been measured on the physical phones.

## Device acceptance

The local evidence directory is
`/Volumes/wavlink/dev/prns-messaging-20260924`. Its source manifests, per-platform
artifact records, build logs and public retained-data summaries identify the
actual packages. Private app-container captures remain outside the repository.
Both packages embed JavaScript, disable the developer TCP fixture and load one
aggregate PRNS native image. Android supports API 29 and its APK/native libraries
pass 16 KiB alignment checks. USB remained attached for installation, observation
and control; there was no Metro or TCP application transport. USB-unplugged
acceptance is not claimed.

The final iOS package includes the contact-upgrade UI, Apple stale-session
recovery and notification-backpressure repair (`e745f3bad`). Its embedded
JavaScript SHA-256 is
`0bc988cbe89557be994c2e928b262da3dcbf3f204ea69bd3dccda056c4eb55ff`;
the signed executable SHA-256 is
`b2ad83823765be3fd634b05233ce3785504c7762609ea0b127a4fa55023ca89b`.
Its retained-data installation passed, but physical message checks below used
the preceding iOS executable
`638279bac07dd794b4b2ccae61cbc4950fd0eb75208947348f0f53208737d36b`
with identical embedded JavaScript. They do not qualify the final Apple repair.
The contract fingerprint is
`prns-app-native/local-node-1/8d5745f8970c48b8`.
The final Android APK includes the same UI and the client-write serialization
repair. Its SHA-256 is
`8d9cf4558d09f1a2e05275fe66c00075be0fa4bde33014fa203c5e53de067d14`.
All 22 packaged native libraries are byte-identical to the initial Android
messaging package; the Android repair changes Kotlin, not the native contract.

Retained installation preserved the iOS identities, two contacts, one pairing
and all 21 original messages exactly. Android preserved its identities, pairing
and 11 original messages. After the final iOS installation, all original stable
records and all 27 pre-install messages still matched exactly. iOS now has three
contacts, one pairing and 27 messages (14 in the Android conversation, 13 in the
older conversation). Android has one contact, one pairing and 16 messages (13 in
the iOS conversation, three in the older conversation). The added iOS record
`iOS-C` remains failed after two attempts and is absent on Android; it must be
retried as that same durable record, not replaced by another message.
Distinct names survived cold launch. Both native share sheets opened and were
dismissed without selecting a recipient. iOS with app Bluetooth disabled reported
**No usable connection** for Announce, then returned to enabled operation.

The initial run found two distinct transport problems. A fresh Apple handshake
could target the closed control receiver of a settled session, returning ATT
error 17. A deterministic regression reproduces that condition; the device log
did not identify the first rejected characteristic, so it does not establish
that every observed error 17 had this cause. After the fix, the observed error-17
loop stopped. An old Android member still required app Bluetooth Off/On to
retire; generic same-peer keeper policy has no settled-session age timeout.

After that reset, Android admitted a link request and received its reply, then
submitted 88-byte and 180-byte data frames consecutively. Android rejected the
second write with result 257. Writes without an ATT response bypassed the shared
client-operation lane. The fix serializes all writes until a matching Android
completion, retains queued data while busy and closes on failed callbacks even
without a matching pending write. Completion and terminal closure share the
admission lock. Legacy result 257 remains terminal because it does not identify
the rejection reason; the log sequence is consistent with the serialization
defect but does not expose Android's internal cause. Seven regression scenarios
exercise the production admission/completion helpers against a platform-busy
model; no Android before-fix test run is claimed. Missing OS completion or
disconnect callbacks remain outside this repair, and no keeper timeout or
reconnect policy changed. The original outgoing record remained the same through
four failed attempts and a final successful explicit retry, with no duplicate
record or automatic resend. Current connection counts alone were not treated as
delivery evidence.

After the stale-session and Android-write repairs were installed, resetting app
Bluetooth on both phones established a fresh session. iOS discovered and saved
the Android name with Automatic Bluetooth ingress and one hop. `iOS-A` reached Delivered in
96 ms and was received on Android. Retrying the original Android record reached
Delivered in 96 ms with the same message ID. No replacement message was created.
Both phones then cleared a nonempty discovery list, retained their saved contact
and sent successfully from Saved: Android in 96 ms and iOS in 92 ms.

Android subsequently cold-launched with an empty discovered list and retained
name/contact. A new Saved send issued a path request, accepted the response and
reached Delivered in 93 ms without a remote manual announce or Bluetooth reset.
iOS cold launch also retained its name, saved contact and messages, with empty
discovery, but Android retained the old settled connection and rejected new
connections from that same identity. The iOS lookup timed out, kept the unsent
draft visible and explicitly said the message had not been queued. This is a
failed seamless-reconnect check, not successful bilateral cold-launch acceptance.

Resetting Android's app Bluetooth allowed that iOS draft to resolve and enter
the durable mailbox, but its delivery timed out. A retry after resetting both
phones also stalled after link identification. Code review found a separate
Apple notification bug: a rejected `updateValue` discarded the fragment while
the data writer returned success. The fix retains that exact fragment and waits
for CoreBluetooth's existing ready callback, with one active fragment per session
and a five-second bound. It checks the original session token before each
attempt, wakes on session/radio retirement, and returns an error on timeout or
closure. Queued work is skipped after cancellation; an OS update already
executing cannot be retracted. The observed payload boundary is consistent with
that bug, but a device notification-admission trace did not prove causation.
The new iOS release package built and installed successfully. Its launch was
denied because the phone was locked, and iPhone Mirroring timed out on two
reconnection attempts. The final explicit retry of `iOS-C` and a fresh Saved send
therefore remain pending. The automated notification regressions pass, but the
physical payload stall is not yet confirmed fixed.

| Check on the messaging builds (before the final Apple notification repair) | iOS | Android |
| --- | --- | --- |
| Retained-data installation and matching source/artifact record | Passed | Passed |
| Distinct names saved and retained after cold launch | Passed | Passed |
| Manual reciprocal announce, discovery details and Save contact | Passed | Passed |
| Select recipient, send and reply without entering hashes | Passed from Discovered and Saved | Passed from Saved |
| Proof-backed delivery and retained conversations | Passed | Passed, including retry of the original record |
| Fresh send after discovery clear and after restart without manual remote reannounce | Clear passed; automatic cold-launch reconnect failed | Both passed |
| My address share sheet remains separate from network announce | Passed | Passed |
| Keyboard-visible name/message editing and enlarged text | Maximum-text profile editor and Save passed; software keyboard unqualified | 1.5x/2x text and visible compose text above keyboard passed |
| Connection interruption gives an honest retryable result without automatic resend | Cold-launch lookup failure retained an unqueued draft | Failed attempts visible; same stored record retained and explicitly retried |

The iOS maximum-accessibility-text check used the Mirroring hardware keyboard.
The longer name saved and survived cold launch; the editor and Save control were
reachable by scrolling. Android compose text remained visible with its software
keyboard open at 1.5x and 2x; Send was reachable after keyboard dismissal. These
checks do not qualify the complete messaging journey at maximum text size.
The original text settings were restored on both phones, including Android 1.1
and iOS's original normal-size slider position with larger accessibility sizes
off. The iOS app Bluetooth preference remains On; the final installed app has
not yet been relaunched. Android app Bluetooth was restored On and reports Ready
with no connected peer while iOS is unavailable; its OS Bluetooth remains On.

## Required Bluetooth lifecycle follow-up

Settled link retirement remains an open cross-target ownership issue. Control
reception belongs to the handshake and is dropped when the link becomes a data
source/sink. A Close arriving after Welcome can therefore be buffered and
discarded or arrive without a settled consumer. Android control submission
acknowledges native-queue admission, so immediate cleanup can also discard an
outgoing Close. Sending a Close immediately before rejecting a duplicate alone
does not provide reliable remote teardown; no such partial fix is included.

The follow-up needs bounded control ownership after the handshake, platform
send/cleanup ordering and protection against late callbacks affecting a
replacement connection. It must cover both keeper rejection and stale incumbents,
including the interactions with fresh-Hello session replacement. Apple app-level
Bluetooth Off currently clears local sessions while preserving its published
GATT service; it does not force a remote central to disconnect. Ordinary shutdown
also has no truthful existing close reason. Graceful shutdown semantics and
missing-callback recovery need an explicit design and device tests, preserving
the current keeper/authentication policy rather than simply evicting every
incumbent for a new challenger.

General network/activity views, historical per-message transport attribution,
locked/background and restoration behavior, permission recovery, long idle and
two-iPhone operation remain separate qualification or later work.

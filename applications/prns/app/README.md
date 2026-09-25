# Prns development app

The Expo app presents the application-owned Rust services. Start with the
[workspace guide](../../README.md) for installation, ownership, generation and
checks; use the [iOS](../../docs/ios.md) or [Android](../../docs/android.md) guide
for device setup. The [validation summary](../../docs/validation.md) separates
current source behavior from build-specific device evidence.

## Current features

The native iOS and Android providers support identity creation/import, local
node inspection, contacts, RemoteControl pairing and connection checks, node
address sharing, expanded node read/write controls, and a persistent mailbox for
small direct LXMF messages.

**Nodes → This phone → Connections** shows local Bluetooth state, connected
physical peers and their traffic/details. Its stored enable/disable setting
controls the existing Bluetooth supervisor without restarting the node or
clearing saved data. Ready with no connected devices is distinct from a live
connection; physical peers are not routes, contacts or board pairings. iOS uses
the native radio state, and Android also checks platform permissions, radio and
required location services, with recovery actions.

Android also exposes service-owned Stop/Start and connection-notification
controls. Web renders the shell with an explicitly unavailable native runtime;
there is no browser or Tauri application provider.

The [Connections checkpoint](../../checkpoints/2026-09-24-local-bluetooth.md)
records retained-data iOS/Android installs, persisted app-off cold launches and
reciprocal physical-peer reconnect. Android OS-radio recovery and 1.5x/2x text
also passed. Final layout builds passed Android 2x and iOS maximum accessibility
text checks, retained saved data and each reported one physical peer; the checkpoint
distinguishes the checks repeated after those layout changes. iOS radio recovery,
permission-denial trials and broader background/restoration qualification remain
open. No new message-delivery or USB-unplugged acceptance is claimed for this
Connections slice. The messaging slice below has separate acceptance in the
[two-phone demo plan](../../docs/phone-node-demo.md). Earlier transport checks do
not qualify the new controls or broader background/restoration behavior.

**Contacts** now owns the editable messaging name, My address sharing and explicit
**Announce yourself**. Saving a name updates future announces and path responses;
it does not broadcast immediately or change cryptographic identity. Saved contacts
preserve private aliases separately from announced names. **Discovered** lists
only authenticated LXMF destinations with last-heard age and ingress details,
bounded to 256 entries and 24 hours per native generation. Clear discovered hides
that history without deleting saved contacts or messages. An authenticated
discovery can upgrade a manually saved address through **Save messaging contact**,
preserving its private name and pin setting.

**New message** selects saved messaging contacts or discovered recipients; manual
address entry remains available. Inbox lists actual conversations. A fresh send
can resolve missing recipient metadata through a bounded native path lookup,
including after restart, before committing a message. It checks saved identity
associations and current stamp requirements; **Finding contact…** is not durable
acceptance. Existing proof-backed delivery, explicit retry and cancellation remain
unchanged. The [messaging checkpoint](../../checkpoints/2026-09-24-messaging-discovery.md)
records reciprocal delivery, discovery-clear sends and Android cold-launch
resolution before the final Apple notification repair. That repair is installed,
but its physical retry remains pending device access. Automatic iOS cold-launch
recovery remains blocked by a stale Bluetooth peer at the other phone; the lookup
keeps the draft unqueued.
Settled Bluetooth link recovery is the next prerequisite in the demo plan.

Pairing is authorization, not a live connection. Contacts and mailbox rows are
local application records, not proof that their peer is reachable. Development
data is resettable and is not promised to survive incompatible upgrades.

The current integration is based on upstream `8c211827b` and represents all 30
RemoteControl request kinds. New board pairings grant Administrator authority
and the board's exact supported request set. Confirmation shows a short access
summary and keeps administrator powers visible; exact controls and the node ID
are available under **Show pairing details**. This is a single preset, not a
permissions picker. Test devices may be
reset and paired again; no deployed-pairing migration is needed.

The expanded controls have host tests and limited Android/board read/write evidence,
not complete physical qualification on either platform. See the
[settings checkpoint](../../checkpoints/2026-09-21-remote-settings-workflows.md)
for the exact builds, checks and remaining limits.

## Route parameters

The 33 stable screen IDs remain in the [navigation catalog](src/navigation/catalog.ts).
Implemented slices use the identifiers their services actually expose:

- onboarding uses `create` and `import` steps;
- contact details, conversations and composition use a destination hash;
- manual contact creation has no source parameter; and
- a managed node's `nodeId` is its target identity fingerprint.

If services add independent record IDs, update catalog parameter types, links
and deep-link tests together. Placeholder routes are not implemented features.

## Connection readiness and cancellation

Before opening a control Link, the native actor resolves the authorized target
and checks its route's actual interface for online, transmit-capable status.
Describe uses the remainder of one 20-second deadline starting at admission,
including queueing, discovery, connection and response time. Address sharing
keeps its separate five-second readiness wait.

A ready route skips discovery. Otherwise, an available transmitting interface
permits one public Prns path request for the exact authorized destination.
After the accepted announcement, the app checks interface readiness again.
A stored route alone is insufficient. This relies on the status published by
the current Bluetooth/TCP transports, not a universal readiness guarantee for
custom interfaces.

Deadline expiry, caller cancellation or priority Stop drops the app-owned
Describe future; expired queued commands never begin network work. Once a target
connection is returned, a guard queues Link closure when that scope ends.
Earlier upstream establishment/identification may already have issued work
before the handle exists. Neither discovery nor the remote command is replayed
automatically.

The managed-node screen cancels pending connection and management reads on navigation,
target changes and native lifetime changes. Provider release and explicit Stop
also cancel caller-owned reads. The SDK forwards `AbortSignal`, including Effect
interruption. Discarding a Promise is not cancellation, and cancellation cannot undo an accepted durable
write or independently spawned work. The [binding ownership guide](../native-composition/bindings/README.md#cancellation-and-ownership)
is the canonical explanation of caller and native-lane ownership.

## Sharing a paired node's address

**Share node address** becomes available after a successful check confirms the
pairing allows it. Native code rechecks current permissions before asking the
target to announce its configured self-announcement destination; that may differ
from its remote-control endpoint.

Submission immediately returns an operation ID. The native actor retains its
latest result across screen remounts and native Stop/Start in the same process.
It is not durable history: process termination or development reset clears it.
If confirmation is lost, the UI reports an unknown outcome and never
automatically repeats the request; the target may already have announced.

## Reading and changing a paired node

Open **Nodes → Manage node**. Settings load automatically. **Interfaces** selects
a connection and exposes its settings and peers; **Device** contains Wi-Fi,
display, positioning and power actions where supported; **Information** shows
firmware and battery/power observations; **Access** lists devices with access.
Technical IDs and connection checks are under **Show node details and diagnostics**.
Lists load on request with bounded pagination. Refresh reads the node again rather
than treating earlier observations as live state.

Controls appear only when the node advertises them and the pairing permits
them. The first slice includes interface power/mode/group, typed LoRa settings,
discovery-group replacement, positioning, display visibility/auto-off, system
sleep/wake, station uplink, Bluetooth/hotspot mode and radio sleep/wake. Setters
without current-state getters are actions rather than switches with guessed
values. The existing keyboard-aware screen and fields are shared by these forms.

Rust validates and admits each change, rechecking live access and capabilities.
Native operation IDs and status distinguish applied, unchanged, scheduled,
failed and uncertain results. A lost response does not automatically replay a
write; leaving the screen does not undo an accepted change. Disruptive actions
require confirmation and explain how the connection may be lost. A wake request
cannot reach a radio over a connection that has already gone offline.

Wi-Fi setup offers **Try network**, then **Keep this network** or **Restore previous
network**. The node owns the confirmation deadline and restores an unconfirmed
trial. Only the node can confirm Wi-Fi readiness; Bluetooth reachability is not
proof. Lost replies require a status check, never automatic credential replay.
Passwords are not stored in operation records or retry queues. Reopening the
screen recovers the node's current state, not durable app-side trial history.

Access inventory reports device IDs only. This phone cannot remove itself;
protected administrators cannot be removed remotely. Adding another controller
still needs verified identity exchange and recipient-side setup; it is not
advertised as an implemented feature. See the
[remaining expansion plan](../../docs/remote-control-expansion.md).

## Platform behavior and next work

The [iOS guide](../../docs/ios.md) covers Bluetooth authorization, process-owned
lifetime, bounded background/restoration behavior, signing and Metro setup.
The [Android guide](../../docs/android.md) covers service controls, permissions,
builds and the physical acceptance procedure.

The [roadmap](../../docs/roadmap.md) is the current implementation backlog.
Historical walking skeletons and diagnostic logs are evidence, not a second
architecture or setup authority.

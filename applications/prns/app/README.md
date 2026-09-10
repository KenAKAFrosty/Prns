# Prns development app

The Expo app presents the application-owned Rust services. Start with the
[workspace guide](../../README.md) for installation, ownership, generation and
checks; use the [iOS](../../docs/ios.md) or [Android](../../docs/android.md) guide
for device setup. The [validation summary](../../docs/validation.md) separates
current source behavior from build-specific device evidence.

## Current features

The native iOS and Android providers support identity creation/import, local
node inspection, contacts, RemoteControl pairing and connection checks, node
address sharing, and a persistent mailbox for small direct LXMF messages.
Android also exposes service-owned Stop/Start and connection-notification
controls. Web renders the shell with an explicitly unavailable native runtime;
there is no browser or Tauri application provider.

Pairing is authorization, not a live connection. Contacts and mailbox rows are
local application records, not proof that their peer is reachable. Development
data is resettable and is not promised to survive incompatible upgrades.

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

The managed-node screen cancels its pending connection check on navigation,
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

## Platform behavior and next work

The [iOS guide](../../docs/ios.md) covers accessory authorization, process-owned
lifetime, bounded background/restoration behavior, signing and Metro setup.
The [Android guide](../../docs/android.md) covers service controls, permissions,
builds and the physical acceptance procedure.

The [roadmap](../../docs/roadmap.md) is the current implementation backlog.
Historical walking skeletons and diagnostic logs are evidence, not a second
architecture or setup authority.

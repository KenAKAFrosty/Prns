# Two-phone local-node demo plan

Status: proposed next product slice, narrowed to BLE-only and automatic,
unpaired peer discovery after review of `22be8b94c` on September 22, 2026.
This is a design plan, not implemented behavior or permission to begin
implementation. Both test phones were visible over USB during the review; no
pairing, announces, messages, installs or connection settings were changed.

## Outcome

Open prns on MetalbeardMobile and the Galaxy S9+, connect them, announce distinct
messaging names, discover and save each other without copying hashes, and exchange
short LXMF messages in both directions. Each phone should explain its connection,
what it has heard, its current routes, and the evidence behind message delivery.

Focus only on Bluetooth: no TCP configuration, relay, board or cross-transport
fallback in this milestone. After normal app-level Bluetooth/platform permission
and enabling the interface, compatible phones should discover and connect
automatically, without choosing or authorizing each phone in a system picker,
OS bonding, or RemoteControl pairing. USB and Metro must not be part of the
communications path. Android 10 remains supported. TCP is deferred.

Foreground success is the first checkpoint, not a foreground-only product
decision. Preserve native background ownership and follow with separately
recorded locked/background/recovery trials. Do not trade away pair-free discovery
silently to improve a particular iOS restoration scenario.

## Current state and gaps

The existing foundation is useful; this is not a messaging-engine rewrite.

| Area | What exists | Gap for the demo |
| --- | --- | --- |
| Local node | One process-owned Rust runtime and stable primary identity | iOS startup currently depends on per-accessory authorization rather than ordinary app-level Bluetooth permission |
| Announcing | Inbox → Messaging options → Share messaging address calls the real LXMF announce API | Hidden, ambiguous label; hardcoded name `prns`; no useful connection/outcome context |
| Discovery | Authenticated LXMF announce observer and latest-per-destination peer cache | Peers appear as empty Inbox conversations; no dedicated discovered-contacts or announce view; cache lacks expiry/cap |
| Contacts | Saved/manual contacts and Save as contact in local diagnostics | No direct discovery-to-contact-to-message journey; generic destinations are not necessarily messaging addresses |
| Bluetooth | iOS central-only; Android scans and advertises | No useful local connection controls or physical-peer view; generic Disconnected label |
| TCP (deferred) | Optional developer TCP client fixture | Not part of this milestone |
| Inspection | Logical interfaces, counters, routes and identity associations | Raw labels/IDs/times; physical peers are folded away; current route is not historical message evidence |

Source anchors: [Inbox](../prns/app/src/features/inbox/inbox-screen.native.tsx),
[local-node views](../prns/app/src/features/nodes/nodes-screen.tsx),
[contacts](../prns/app/src/features/contacts/contacts-screen.tsx),
[native composition](../prns/native-composition/src/lifecycle.rs),
[LXMF peer owner](../services/lxmf/src/direct.rs),
[durable messaging owner](../services/lxmf/src/mailbox.rs), and
[iOS admission](../sdk/expo/ios/PrnsAccessorySetupCoordinator.swift).

The historical local `scratch/prns-app/product-and-ux.md` already called
for Saved/Discovered contacts, local-node-first navigation, local interfaces and
network inspection. Restore those product priorities, not its superseded bridge
designs or speculative service registries. The tracked roadmap remains the
current backlog; old physical results do not qualify new binaries.

## Transport decision: automatic connections without pairing

Distinguish four mechanisms:

- App-level permission allows prns to use Bluetooth; this normal OS consent stays.
- A BLE connection is a transport session; it does not itself imply pairing or
  bonding, and the app can manage it automatically.
- OS pairing/bonding establishes Bluetooth-layer security keys; the current Prns
  GATT transport does not require it.
- AccessorySetupKit authorizes particular accessories to this app. Even without
  an OS bond, its per-peer picker is incompatible with the requested discovery UX.

Prns AutoBLE already supports pair-free connections. Android uses ordinary GATT
permissions and insecure L2CAP sockets, without a bonding call. Apple uses
ordinary GATT permissions; the backend deliberately avoids central-initiated
L2CAP where that would trigger bonding. Prefer the existing GATT path for this
demo, not encrypted-GATT requirements that introduce new system pairing prompts.

The recommended design investigation is ordinary CoreBluetooth authorization
instead of AccessorySetupKit for the app's general-purpose AutoBLE interface.
Apple's [two-device sample](https://developer.apple.com/documentation/corebluetooth/transferring-data-between-bluetooth-low-energy-devices)
demonstrates service-filtered discovery, automatic connection and bidirectional
GATT exchange. Use the existing Prns peer/role/session policy rather than another
phone-only protocol. For the mixed pair, iOS as central and Android advertising
is the preferred role arrangement; central/peripheral roles remain internal.

This requires a new app configuration/admission path, not merely hiding the
picker. The current installed app still uses AccessorySetupKit and central-only
Bluetooth. The [iOS guide](ios.md) records a historical abort when constructing a
peripheral manager with AccessorySetupKit active; it does not establish that
ordinary non-ASK dual-role CoreBluetooth is impossible. The public
`AutoBle::prepare_with_restoration` API already supports both managers, with
stable app-owned restoration identifiers. Re-evaluate that path without ASK;
do not disable restoration or propose an unverified runtime toggle between
incompatible permission modes. Core role selection permits iOS-to-iOS GATT too,
but this has not been qualified with two physical iPhones.

Background operation is not synonymous with relaunch after force-quit. Apple's
[TN3115](https://developer.apple.com/documentation/technotes/tn3115-bluetooth-state-restoration-app-relaunch-rules)
attaches the iOS 26 ASK condition to specific relaunch cases; an
[Apple engineering clarification](https://developer.apple.com/forums/thread/818370)
distinguishes ordinary state restoration from the force-quit exception. Ordinary
CoreBluetooth retains background/restoration mechanisms; do not promise relaunch
after force-quit without ASK or describe removing ASK as necessarily foreground-only.
Validate each supported state on the current OS instead of transferring old ASK
test results to the new permission model.

iOS background peripheral advertising omits the local name and puts service UUIDs
in an overflow area discoverable only by scanning iOS devices
([Apple documentation](https://developer.apple.com/documentation/corebluetooth/cbperipheralmanager/startadvertising(_:))).
This is another reason to favor iOS scanning an Android advertiser for cross-
platform background discovery. Test initial discovery separately from maintaining
or restoring an existing connection. Two-iPhone operation is a later physical
qualification target, not proven by the available mixed pair.

Treat automatic BLE peers as untrusted network access, not trusted contacts or
remote controllers. Preserve Reticulum/LXMF cryptographic checks and separate
RemoteControl grants; service UUIDs and BLE identifiers are not person identity.
Public announces remain public, and automatic connection exposes discovery and
traffic metadata. Keep peer/queue limits, handshake timeouts, backoff and an
explicit interface-off control; avoiding bonding does not remove these needs.

The reported Disconnected status is not enough to diagnose a fault. The logical
Bluetooth interface can be ready with zero connected members. Determine which
stage is missing: radio/permission, scanning or advertising, authorization,
connection, Prns handshake, route discovery, or message delivery. USB attachment
does not establish a Bluetooth connection.

## Product model and navigation

Keep existing tabs and stable routes. Add entry points to the existing local
interface/activity routes rather than introducing another top-level tab.

| Location | Proposed experience |
| --- | --- |
| Nodes | This phone first: messaging name, Running/Stopped, concise connection summary; Connections and Network details. Managed boards stay below and visibly separate. |
| Contacts | Saved / Discovered; prominent Announce yourself and a separate My address action for copying/sharing the address. |
| Discovered contact | Announced name, short address, human-readable last heard, received-via connection/hops when known; Save contact and Message. Identity details secondary. |
| Inbox | Actual conversations, not every heard peer. New message selects a saved/discovered recipient; manual address entry remains available. Contact detail also has Message. |
| Connections | Automatic Bluetooth with readable state, app-level permission action, enable/disable, connected peers and connection details. No per-phone picker. Local settings never use remote-board controls. |
| Network details / Activity | Physical connections, current routes, accepted announces and bounded recent connection/message events; filters and clear local history. Technical IDs remain available on demand. |
| Conversation details | Current route availability and independently recorded delivery evidence, clearly labeled. No inferred historical path presented as fact. |

Use Discovered, not Nearby, for announce-derived people: they may be reached over
multiple hops, even when this phone uses only BLE. Reserve Nearby for physical
Bluetooth discovery. Keep these concepts
separate: Bluetooth authorization/connectivity, announcing a messaging address,
and RemoteControl pairing. Messaging does not confer board-administration access.

The local profile owns an editable, persisted messaging name. It is distinct
from the OS Bluetooth name, a contact's private alias and cryptographic identity.
Changing the name must update registered LXMF announce data, including path
responses, without changing identity or discarding messages. Announced names are
untrusted display data, not verified real-world identities.

## Truthful status, announcing and discovery policy

- Show local node state separately from Bluetooth state. Saved contacts and
  mailbox remain readable with Bluetooth disabled or permission denied.
- Present Bluetooth states such as Off, Permission needed,
  Looking for devices, Connecting, Connected to N devices, or Failed with a
  recovery action. Only claim scanning/advertising when actual status exposes it;
  otherwise say Ready, no connected devices. Keep `AutomaticBluetoothLe` in
  technical details rather than the primary heading.
- Physical Bluetooth peers, Reticulum routes, Reticulum Links and contacts are
  different counts. Never derive one from another. A detected peer is not
  necessarily connected; a recent announce is not a live reachability test.
- Make Announce yourself a first-class action with a brief explanation that it
  publishes the messaging name/address onto enabled connected networks. Keep
  network announce distinct from copy/share-sheet actions. Use all eligible
  interfaces initially; transport isolation comes from connection controls.
- Core announce completion currently means accepted for fan-out, not transmitted
  or heard. It can succeed with no usable egress. Report no usable connection
  explicitly and otherwise say Announcement requested unless stronger evidence
  exists. Do not promise discovery on the other phone from this result alone.
- Start with explicit manual announce. Add opt-in automatic announcing on a
  usable connection only as a bounded native policy with cooldown/coalescing,
  privacy disclosure and tests; never tie broadcasts to screen renders/polling
  or add aggressive periodic announcements to make the demo pass.
- Discovered messaging peers are deduplicated by destination, with bounded
  capacity and age expiry; repeated observations update last heard. Keep a
  separately bounded recent accepted-announce feed, including non-LXMF
  destinations, for inspection. Proposed initial bounds: 256 discovered entries,
  24-hour expiry, 200 activity rows; tune with tests rather than expose knobs.
- Discovery/history initially live for the native process, with clear reset
  semantics. Saved contacts, names, connection settings and mailbox are durable.
  Clearing observed history must not delete contacts, revoke permissions, block
  peers or change routing. Filtering is local presentation, not a network ban.
- Separate recent-discovery history from messaging resolution. Fresh sends
  currently require an entry in the process-local LXMF peer cache before a path
  request is attempted, so a saved contact can be unsendable after restart until
  another announce arrives. Resolve saved recipients through a bounded native
  path/announce lookup, respecting identity conflicts and current stamp
  requirements. Show Finding contact and a useful timeout/retry result. Clearing
  or expiring discovery rows must not itself make saved contacts unsendable.
- Offer Save/Message only for recognized messaging destinations; other accepted
  announces remain inspectable. Hide own destinations from Discovered contacts.
  Saving rechecks authenticated identity association and retains conflict rules;
  do not silently replace a pinned identity or overwrite a private alias.

## Architecture and narrow additions

Keep the current ownership: Rust owns node lifetime, profile/configuration,
discovery, commands and durable mailbox; native adapters own OS permissions and
platform lifecycle; generated UniFFI bindings carry typed results; React owns
presentation. No second node, protocol scheduler or JavaScript packet/event bus.

Reuse public AutoBLE composition, `announce_now`, `set_registered_announce_app_data`,
the accepted announce observer, raw interface inventory and engine inspection.
The accepted observer already supplies identity, interface, hops and
monotonic time. Copy bounded data into the existing native owner; never discover
contacts from unauthenticated diagnostic events.

Extend the native projections deliberately: local messaging address/name,
per-interface readiness and physical members, discovery provenance and ages,
bounded activity, and operation outcomes. Keep canonical HostSnapshot unchanged
unless a reusable core consumer needs its vocabulary expanded: it deliberately
folds physical members into logical interfaces. Supplement it from the same raw
capture; do not fork its projection or reconstruct physical peers from totals.
Convert monotonic observations to ages in Rust, not phone wall-clock dates.

Replace the per-accessory startup gate with explicit app-level permission/radio
state while preserving early native restoration preparation, one owner,
Stop/reset cancellation and Android service ownership. Permission denial must
not hide saved data. Add the BLE interface enable/disable control and real
readiness projection without creating a generic multi-interface configuration
framework now. State whether a change reconnects the interface or restarts the
node; never clear data or silently interrupt an accepted operation.

Retain existing inbound interface, arrival and Link ID evidence instead of
discarding it in the mailbox projection. Outbound Link ID and measured RTT can
also be retained app-side, but exact outbound interface attribution needs a
narrow upstream inspection/result seam: current settlement and Host snapshots
do not expose it. Propose that extension with tests on a separate upstream branch
if exact outbound path display is required; until then label it unavailable.
Never attach today's route to an old message and call it its delivery route. A
known isolated test plus counters is test evidence, not a general per-message
tracing feature.

If per-message connection evidence is retained across restart, persist it with
the corresponding attempt/result. Otherwise label it as session-only and show
Not recorded for older messages. Do not reconstruct missing history from current
routes or counters.

## Ordered implementation slices, after approval

1. **Pair-free connection feasibility and status.** After implementation approval,
   qualify ordinary CoreBluetooth permissions and existing Prns AutoBLE roles
   without ASK. Capture exact builds and show that neither phone needs a bond,
   per-peer chooser or RemoteControl grant. Include permission denial, readiness,
   interface enable/disable and truthful status. Keep native restoration support
   and test its boundaries; stop to revise the design if the pair cannot connect
   without per-peer setup. No board or TCP fallback.
2. **Complete the messaging journey over BLE.** Add persisted name, accessible
   announce, bounded Discovered list, Save/Message actions and contact recipient
   selection. Prove reciprocal discovery and short proof-backed messages without
   typing addresses, including a fresh send to a saved contact after restart
   without manually re-announcing at the other phone. Include basic
   connection/counter evidence from slice 1.
3. **Network explainability.** Complete announce/activity inspection, readable
   route age/expiry and physical peer views, and conversation delivery details.
   Add a narrow upstream seam only where existing public evidence cannot answer
   the UI's question. Clearly separate current routes from actual message paths.
4. **Repeatable device acceptance and recovery.** Test out-of-range, reconnect,
   restart, Stop/Start and permission denial. Follow with background/locked trials,
   ordinary restoration versus force-quit, and OS-specific limitations. Check real
   keyboards, retained data and 1.5x/2x text on both phones.

For each slice: focused native/SDK/UI tests, regenerated bindings where needed,
retained-data builds, exact binary/device evidence and a logical commit. Keep
generic upstream fixes on separate branches. No publishing is implied by this
plan, and no historical phone test substitutes for the new two-phone journey.

## Demo acceptance and deferred scope

The demo passes when a user can complete the following using only the app UI:

1. See each phone's distinct name and whether it has a usable connection.
2. Connect iPhone/Galaxy automatically over BLE after app-level permission, with
   no OS bond, per-peer system picker, RemoteControl grant or board.
3. Announce both ways, see each other under Discovered and save contacts.
4. Send and reply; show Delivered only on the existing proof-backed result,
   distinguish it from read status, and preserve messages after restart. A saved
   contact remains usable through native resolution after discovery history is
   cleared or the app restarts; failure must give a useful reason, not require
   unexplained manual announce rituals.
5. Inspect the actual BLE peers, relevant routes, heard announces and traffic.
6. Recover from one interrupted connection with clear state and no duplicate
   messages or automatic replay of an uncertain user operation. Reuse the
   existing explicit retry/cancel policy rather than inventing fallback sends.

Background receipt, first request after wake and OS restoration receive their
own pass/fail records; foreground demo success must not be called full background
qualification. Preserve background support as a requirement, not a guarantee of
an always-running iOS daemon.

Defer TCP and all other transport configuration, LXMF Resources/attachments,
propagation, opportunistic delivery, push notifications, full persistent packet
history, graph visualizations,
multi-endpoint contact merging, identity export/sync, new desktop/browser
providers and additional board-control features. Existing board qualification
and firmware faults remain tracked separately; they do not gate a board-free
phone-to-phone demo unless a shared dependency is actually involved.

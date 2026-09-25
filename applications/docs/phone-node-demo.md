# Two-phone local-node demo plan

Status: the local Connections slice passed bounded retained-data iOS/Android
acceptance on September 24, 2026. The [checkpoint](../checkpoints/2026-09-24-local-bluetooth.md)
records persisted app-off cold launches, retained data and reciprocal physical
connections on that slice's final builds, plus Android radio recovery and
larger-text checks on both platforms. iOS radio recovery, permission-denial trials
and broader lifecycle qualification remain open.
Persisted messaging names, bounded Saved/Discovered contacts, recipient selection
and native recipient resolution are implemented. The
[messaging checkpoint](../checkpoints/2026-09-24-messaging-discovery.md) records
reciprocal discovery, proof-backed delivery, discovery-clear sends and Android
cold-launch sending before the final Apple notification repair. Automatic iOS
cold-launch recovery is blocked by a stale Bluetooth member retained by the other
phone; fix settled link ownership before adding broader network inspection.
The latest Apple notification repair passed the retained-message retry, fresh
Saved send and reverse delivery on September 25. The additional cold-restart
check was not started before the phone became unavailable. Receiver source-key
retention and verification also need follow-up; delivery proof does not establish
the sender's identity. The user accepts losing ASK's extra force-quit relaunch
support to make the phone usable as its own node,
without a board or per-peer authorization.
Control Center Bluetooth-toggle recovery is another documented ASK difference;
physical results must record it separately from ordinary background operation.

The [September 23 physical checkpoint](../checkpoints/2026-09-23-ordinary-corebluetooth.md#physical-continuation-direct-unpaired-ble-messaging)
records automatic unpaired iOS/Android connection, reciprocal announcements,
two-way proof-backed messages, one same-process off-screen iOS receipt and its
first resumed send on earlier binaries. That establishes transport feasibility;
it does not substitute for the current Connections checkpoint or complete demo
acceptance.

## Outcome

Open prns on iOS and Android, connect them, announce distinct
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
| Local node | One process-owned Rust runtime and stable primary identity; ordinary iOS Bluetooth admission; This phone appears before managed boards; retained-data cold launches passed on both platforms | Permission recovery and full physical lifecycle qualification remain |
| Announcing | Contacts exposes a persisted messaging name, explicit Announce yourself and separate My address sharing; reciprocal messaging-build discovery passed | Wider announce/activity inspection remains planned |
| Discovery | Discovered contacts shows accepted LXMF names, age, ingress and hops; 256-entry capacity, 24-hour expiry and explicit clear | General accepted-announce activity inspection remains planned |
| Contacts | Save / Message from discovery; saved messaging recipients, private aliases and announced names remain distinct; Inbox contains message-backed conversations; discovery-clear sends passed on both platforms | Android cold-launch send passed; iOS cold-launch send is blocked by Bluetooth recovery |
| Bluetooth | Connections shows local status, stored enable/disable and physical peers; final Apple notification repair passed retained-message retry and fresh-session two-way delivery | Stale settled peer after iOS restart remains an open blocker; iOS OS-radio recovery, permission denial, background and long-idle qualification remain |
| TCP (deferred) | Optional developer TCP client fixture | Not part of this milestone |
| Inspection | Logical interfaces, counters, routes and identity associations; Connections adds physical Bluetooth peers and their counters/details | Broader network views still use raw labels/IDs/times; current route is not historical message evidence |

Source anchors: [Inbox](../prns/app/src/features/inbox/inbox-screen.native.tsx),
[local-node views](../prns/app/src/features/nodes/nodes-screen.tsx),
[Connections](../prns/app/src/features/connections/connections-screen.tsx),
[contacts](../prns/app/src/features/contacts/contacts-screen.tsx),
[native composition](../prns/native-composition/src/lifecycle.rs),
[LXMF peer owner](../services/lxmf/src/direct.rs),
[durable messaging owner](../services/lxmf/src/mailbox.rs), and
[iOS admission](../prns/platform/ios/PrnsBluetoothCoordinator.swift).

The historical local `scratch/prns-app/product-and-ux.md` already called
for Saved/Discovered contacts, local-node-first navigation, local interfaces and
network inspection. Restore those product priorities, not its superseded bridge
designs or speculative service registries. The tracked roadmap remains the
current backlog; old physical results do not qualify new binaries.

## Implemented Connections slice

Open **Nodes → This phone → Connections** or **More → Connections**. Node state
and Bluetooth state are separate: a running node can have Bluetooth turned off,
and a Bluetooth connection status of Ready means no physical peers are currently
connected. Connected counts and peer details include only connected Bluetooth
fleet members, never routes,
Reticulum Links, contacts or managed-board pairings. Stale peer rows are hidden
while access is unavailable or the interface is turning off.

The app stores the Bluetooth preference through its existing native database
owner, then applies it through the shared AutoBLE supervisor. Toggling this
interface does not restart the node or clear identity, contacts or messages.
Startup reads the same stored preference. An unconfirmed setting result asks for
a refresh; it does not imply that an accepted write was rolled back.

The [app Bluetooth projection](../prns/native-composition/src/bluetooth.rs) uses
physical fleet facts from the same native Host inspection capture that produces
the canonical logical HostSnapshot. It does not duplicate that projection or
infer physical connections from its aggregate totals. iOS status includes the
real CoreBluetooth radio state; Android status combines supervisor facts with
platform permission, radio and required location-service gates. Unknown or
failed access checks do not count as ready.

The [September 24 checkpoint](../checkpoints/2026-09-24-local-bluetooth.md) records
retained-data installation, app-off cold-launch persistence and reconnect on
both platforms, plus Android OS-radio recovery and 1.5x/2x text checks. Final
layout builds passed Android 2x and iOS maximum-accessibility-text checks; the
checkpoint separates those follow-up checks from the original control trials.
iOS radio recovery, permission-denial trials and
background/restoration qualification remain open. Messaging names and the
Saved/Discovered journey are implemented separately below.

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

The approved feasibility implementation uses ordinary CoreBluetooth authorization
instead of AccessorySetupKit for the app's general-purpose AutoBLE interface.
Apple's [two-device sample](https://developer.apple.com/documentation/corebluetooth/transferring-data-between-bluetooth-low-energy-devices)
demonstrates service-filtered discovery, automatic connection and bidirectional
GATT exchange. Use the existing Prns peer/role/session policy rather than another
phone-only protocol. For the mixed pair, iOS as central and Android advertising
is the preferred role arrangement; central/peripheral roles remain internal.

This requires a new app configuration/admission path, not merely hiding the
picker. Installation and acceptance of this new path must be recorded separately
from earlier ASK binaries. The [iOS guide](ios.md) records a historical abort when constructing a
peripheral manager with AccessorySetupKit active; it does not establish that
ordinary non-ASK dual-role CoreBluetooth is impossible. The public
`AutoBle::prepare_with_restoration` API already supports both managers, with
stable app-owned restoration identifiers. Qualify that path without ASK;
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

| Location | Implemented / planned experience |
| --- | --- |
| Nodes | Implemented: This phone first, Running/Stopped, concise Bluetooth summary and Connections; managed boards below. Edit the messaging name in Contacts. Expanded Network details remain planned. |
| Contacts | Saved / Discovered; prominent Announce yourself and a separate My address action for viewing/sharing the address. |
| Discovered contact | Announced name, short address, human-readable last heard, received-via connection/hops when known; Save contact and Message. Identity details secondary. |
| Inbox | Actual conversations, not every heard peer. New message selects a saved/discovered recipient; manual address entry remains available. Contact detail also has Message. |
| Connections | Implemented with bounded iOS/Android retained-data acceptance: automatic Bluetooth, readable state, app-level permission actions, stored enable/disable and physical peer details. Remaining recovery/lifecycle checks are listed in the checkpoint. No per-phone picker; local settings do not use remote-board controls. |
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
  capacity (256 entries) and monotonic age expiry (24 hours); repeated observations
  update last heard. A separately bounded recent accepted-announce feed, including
  non-LXMF destinations, remains planned for inspection (initial proposal: 200
  activity rows). These are internal bounds, not user configuration.
- Discovery lives for the current native node generation; Stop/Start or process
  restart clears it. The planned activity history needs similarly explicit reset
  semantics. Saved contacts, names, connection settings and mailbox are durable.
  Clearing observed history must not delete contacts, revoke permissions, block
  peers or change routing. Filtering is local presentation, not a network ban.
- Separate recent-discovery history from messaging resolution. Fresh sends
  use authenticated cached metadata when still valid; otherwise a native
  path/announce lookup waits at most 15 seconds before durable acceptance. At
  most eight lookups wait concurrently. A cached route alone is insufficient.
  Identity conflicts, unsupported stamp requirements and metadata that expires
  while awaiting admission reject before a message is queued. The UI shows
  Finding contact and a retryable failure. Clearing discovery hides observations
  while preserving bounded recipient metadata; neither clearing nor restart
  requires a manual remote announcement to initiate lookup.
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

The ordinary Bluetooth startup path uses app-level permission/radio state while
preserving early native restoration preparation, one owner, Stop/reset
cancellation and Android service ownership. The implemented BLE control and
readiness projection reuse that ownership; permission denial leaves saved data
available. Keep further transport configuration out of this slice. Disabling
Bluetooth disconnects its physical peers without restarting the node; it is
independent of stopping the node or resetting app data.

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

## Ordered implementation slices

1. **Pair-free connection feasibility and status (bounded device acceptance passed).**
   Earlier builds established automatic unpaired transport feasibility. The
   Connections UI, stored interface control and physical-peer projection are now
   implemented. The September 24 checkpoint records retained data, app-off cold
   launches and reconnect on both platforms, plus Android radio recovery and
   larger-text checks on both platforms. Complete the listed iOS radio and
   permission-denial gaps;
   keep background/restoration and USB-unplugged trials separately bounded and
   recorded. This slice did not repeat message-delivery acceptance.
2. **Complete the messaging journey over BLE (implemented; iOS recovery remains blocked).**
   Persisted name, explicit announce, bounded Discovered list, Save/Message actions
   and contact recipient selection are implemented. Reciprocal discovery and
   proof-backed messages without typing addresses passed, as did discovery-clear
   sending on both phones and Android cold-launch sending without a remote
   manual announce. The messaging checkpoint records a reproducible iOS restart
   failure and honest unqueued-draft feedback.
3. **Settled Bluetooth link recovery (next).** Define bounded control ownership
   after the handshake, send/cleanup ordering, graceful Off semantics and stale
   incumbent recovery. Preserve keeper/authentication policy and late-callback
   fencing. Prove ordinary iOS restart and one-sided app Off/On recover without
   resetting the other phone, then repeat the saved-recipient send check.
4. **Network explainability.** Complete announce/activity inspection, readable
   route age/expiry, broader connection inspection and conversation delivery details.
   Add a narrow upstream seam only where existing public evidence cannot answer
   the UI's question. Clearly separate current routes from actual message paths.
5. **Repeatable device acceptance and recovery.** Test out-of-range, reconnect,
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
2. Connect iOS/Android automatically over BLE after app-level permission, with
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

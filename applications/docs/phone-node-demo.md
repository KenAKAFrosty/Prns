# Two-phone local-node demo plan

Status: proposed next product slice, reviewed against `49279de25` on September
22, 2026. This is a design plan, not implemented behavior or permission to begin
implementation. Both test phones were visible over USB during the review; no
pairing, announces, messages, installs or connection settings were changed.

## Outcome

Open prns on MetalbeardMobile and the Galaxy S9+, connect them, announce distinct
messaging names, discover and save each other without copying hashes, and exchange
short LXMF messages in both directions. Each phone should explain its connection,
what it has heard, its current routes, and the evidence behind message delivery.

Prove Bluetooth-only first, then TCP-only, then coexistence and recovery. The
Bluetooth demo should need neither a board nor a network relay. The first TCP
demo may use an explicitly identified shared Reticulum transport on the LAN;
that is a routed two-phone exchange, not direct phone-to-phone TCP. USB and Metro
must not be part of the communications path. Android 10 remains supported.

Foreground success is the first checkpoint, not a foreground-only product
decision. Preserve native background ownership and follow with separately
recorded locked/background/recovery trials. TCP does not inherit a Bluetooth
background-execution entitlement.

## Current state and gaps

The existing foundation is useful; this is not a messaging-engine rewrite.

| Area | What exists | Gap for the demo |
| --- | --- | --- |
| Local node | One process-owned Rust runtime and stable primary identity | iOS startup depends on an authorized Bluetooth accessory even for future TCP-only operation |
| Announcing | Inbox → Messaging options → Share messaging address calls the real LXMF announce API | Hidden, ambiguous label; hardcoded name `prns`; no useful connection/outcome context |
| Discovery | Authenticated LXMF announce observer and latest-per-destination peer cache | Peers appear as empty Inbox conversations; no dedicated discovered-contacts or announce view; cache lacks expiry/cap |
| Contacts | Saved/manual contacts and Save as contact in local diagnostics | No direct discovery-to-contact-to-message journey; generic destinations are not necessarily messaging addresses |
| Bluetooth | iOS central-only; Android scans and advertises | No useful local connection controls or physical-peer view; generic Disconnected label |
| TCP | Optional developer TCP client fixture | No saved user connection settings; iOS Release excludes the fixture |
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

## Transport decision and first investigation

For these phones, use iOS as Bluetooth central and Android as the advertising
peer. Android's advertised service matches the current iOS picker selector;
Prns's connection policy explicitly supports this role combination. This is
source-level feasibility, not a completed device test. Authorize the Galaxy in
the iPhone's system picker before expecting automatic reconnection. An existing
authorization for a board does not authorize the phone.

With AccessorySetupKit, scans expose authorized accessories, not arbitrary nearby
phones ([Apple's explanation](https://developer.apple.com/videos/play/wwdc2024/10203/)).
The picker can also be hard to identify because Android does not currently
advertise a device name. Check actual picker presentation and advertised payload
on these builds before promising a named-phone chooser.

Two iPhones cannot directly connect under the current central-only composition.
The [iOS guide](ios.md) records an earlier physical abort when combining
AccessorySetupKit and a peripheral manager. General iPhone-to-iPhone BLE requires
a separate platform decision: re-evaluate supported dual-role authorization,
restoration and background behavior; do not casually remove AccessorySetupKit or
just turn on a peripheral flag. Apple's [restoration rules](https://developer.apple.com/documentation/technotes/tn3115-bluetooth-state-restoration-app-relaunch-rules)
and [background advertising constraints](https://developer.apple.com/documentation/corebluetooth/cbperipheralmanager/startadvertising(_:))
make that a real tradeoff. Do not advertise all phone combinations as supported
after qualifying only iPhone/Android.

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
| Connections | Bluetooth and saved TCP connections with readable status, permissions/setup actions, enable/disable and connection details. Local settings never use remote-board controls. |
| Network details / Activity | Physical connections, current routes, accepted announces and bounded recent connection/message events; filters and clear local history. Technical IDs remain available on demand. |
| Conversation details | Current route availability and independently recorded delivery evidence, clearly labeled. No inferred historical path presented as fact. |

Use Discovered, not Nearby, for announce-derived people: they may be reached over
TCP or multiple hops. Reserve Nearby for Bluetooth discovery. Keep these concepts
separate: Bluetooth authorization/connectivity, announcing a messaging address,
and RemoteControl pairing. Messaging does not confer board-administration access.

The local profile owns an editable, persisted messaging name. It is distinct
from the OS Bluetooth name, a contact's private alias and cryptographic identity.
Changing the name must update registered LXMF announce data, including path
responses, without changing identity or discarding messages. Announced names are
untrusted display data, not verified real-world identities.

## Truthful status, announcing and discovery policy

- Show local node state separately from each interface's state. A TCP-only node
  can run with Bluetooth disabled, denied or not yet authorized; saved contacts
  and mailbox remain readable offline.
- Present Bluetooth states such as Off, Permission needed, Choose a device,
  Looking for devices, Connecting, Connected to N devices, or Failed with a
  recovery action. Only claim scanning/advertising when actual status exposes it;
  otherwise say Ready, no connected devices. Keep `AutomaticBluetoothLe` in
  technical details rather than the primary heading.
- Physical Bluetooth peers, Reticulum routes, Reticulum Links and contacts are
  different counts. Never derive one from another. An authorized accessory is
  not necessarily connected; a recent announce is not a live reachability test.
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

Reuse public `announce_now`, `set_registered_announce_app_data`, the accepted
announce observer, raw interface inventory, engine inspection and TCP client
attachment. The accepted observer already supplies identity, interface, hops and
monotonic time. Copy bounded data into the existing native owner; never discover
contacts from unauthenticated diagnostic events.

Extend the native projections deliberately: local messaging address/name,
per-interface readiness and physical members, discovery provenance and ages,
bounded activity, and operation outcomes. Keep canonical HostSnapshot unchanged
unless a reusable core consumer needs its vocabulary expanded: it deliberately
folds physical members into logical interfaces. Supplement it from the same raw
capture; do not fork its projection or reconstruct physical peers from totals.
Convert monotonic observations to ages in Rust, not phone wall-clock dates.

Separate native node admission from individual transport admission. Preserve
early iOS Bluetooth restoration preparation, one owner, Stop/reset cancellation,
and Android service ownership. Permission denial or failure on one optional
interface must not prevent another configured interface from running. Changing
configuration must state whether it reconnects one interface or restarts the
node; never clear data or silently interrupt an accepted operation.

TCP v1 is a saved client connection: name, host, port, enabled state, connection
status and bounded native reconnect. Include input validation and local-network
permission/error UX. Do not automatically enable a public relay. Use two clients
to one explicitly configured transport node for the first TCP demo; a plain
socket listener without Reticulum forwarding is not a relay. A phone-side TCP
listener or LAN auto-discovery is a follow-on, not disguised as already provided
by the development fixture.

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

1. **Connection feasibility and status.** Capture exact installed builds and
   readiness on both phones; test the Galaxy picker/authorization/handshake with
   no board or TCP fallback. Specify missing native status fields and confirm
   the iOS role decision. Finish with a truthful connection card, not an
   unexplained Disconnected label. Stop to revise the transport design if the
   pair cannot establish a direct link.
2. **Complete the messaging journey over BLE.** Add persisted name, accessible
   announce, bounded Discovered list, Save/Message actions and contact recipient
   selection. Prove reciprocal discovery and short proof-backed messages without
   typing addresses, including a fresh send to a saved contact after restart
   without manually re-announcing at the other phone. Include basic
   connection/counter evidence from slice 1.
3. **Independent local connections and TCP.** Decouple whole-node admission from
   Bluetooth; implement saved TCP client configuration and explicit BLE controls.
   Prove TCP-only operation with Bluetooth off and no authorized board, on both
   platforms, without build-time fixture values. Preserve BLE-only acceptance.
4. **Network explainability.** Complete announce/activity inspection, readable
   route age/expiry and physical peer views, and conversation delivery details.
   Add a narrow upstream seam only where existing public evidence cannot answer
   the UI's question. Clearly separate current routes from actual message paths.
5. **Repeatable device acceptance and recovery.** Repeat each isolated transport,
   then both together; test out-of-range, reconnect, restart, Stop/Start and
   permission denial. Follow with background/locked trials and OS-specific
   limitations. Check real keyboards, retained data and 1.5x/2x text on both phones.

For each slice: focused native/SDK/UI tests, regenerated bindings where needed,
retained-data builds, exact binary/device evidence and a logical commit. Keep
generic upstream fixes on separate branches. No publishing is implied by this
plan, and no historical phone test substitutes for the new two-phone journey.

## Demo acceptance and deferred scope

The demo passes when a user can complete the following using only the app UI:

1. See each phone's distinct name and whether it has a usable connection.
2. Connect iPhone/Galaxy directly over BLE, with no RemoteControl grant or board.
3. Announce both ways, see each other under Discovered and save contacts.
4. Send and reply; show Delivered only on the existing proof-backed result,
   distinguish it from read status, and preserve messages after restart. A saved
   contact remains usable through native resolution after discovery history is
   cleared or the app restarts; failure must give a useful reason, not require
   unexplained manual announce rituals.
5. Inspect the actual BLE peers, relevant routes, heard announces and traffic.
6. Repeat using only configured TCP through the named transport; the UI must
   explain that topology. Do not let BLE silently rescue the TCP test or vice versa.
7. Recover from one interrupted connection with clear state and no duplicate
   messages or automatic replay of an uncertain user operation. Reuse the
   existing explicit retry/cancel policy rather than inventing fallback sends.

Background receipt, first request after wake and OS restoration receive their
own pass/fail records; foreground demo success must not be called full background
qualification. Preserve background support as a requirement, not a guarantee of
an always-running iOS daemon.

Defer LXMF Resources/attachments, propagation, opportunistic delivery, push
notifications, full persistent packet history, graph visualizations,
multi-endpoint contact merging, identity export/sync, new desktop/browser
providers and additional board-control features. Existing board qualification
and firmware faults remain tracked separately; they do not gate a board-free
phone-to-phone demo unless a shared dependency is actually involved.

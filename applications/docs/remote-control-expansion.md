# Expanded node management

Implementation status and remaining plan against upstream `8c211827b`, including
[PR #232](https://github.com/KenAKAFrosty/Prns/pull/232) and discovery-group
read/write support. The app names all 30 request kinds and implements its first
expanded read/write slice: node overview, interfaces, configuration, peers,
discovery groups and 13 typed ordinary/disruptive changes. Guided Wi-Fi trials
and controller inventory/removal are now implemented. Authorizing another
controller and the separately advertised legacy Wi-Fi setter remain planned.

The [September 21 checkpoint](../checkpoints/2026-09-21-remote-control-management.md)
records the initial validation. The later
[settings checkpoint](../checkpoints/2026-09-21-remote-settings-workflows.md)
separates host checks from the actual Galaxy/E290 trials. That evidence does not
qualify every control, firmware variant or iOS build.

## Ownership and product rules

Keep the existing architecture. Public PRNS owns the protocol, authorization,
board capability reporting and target-side transactions. Application Rust owns
bounded workflows and operation lifetimes. Generated UniFFI bindings carry typed
data; React Native owns presentation and platform interaction. Do not add a
generic RPC escape hatch, another transport, or a JavaScript command queue.

Show features from the live intersection of board capabilities and the paired
controller's permissions, not from board model names. Unsupported controls are
absent or clearly unavailable, never silently simulated. Keep user-facing wording
free of protocol details. Recheck authorization and capabilities before a write.

Upstream enforces both authority and an allowed request set. Administrator
authority permits access management; ordinary settings still require their
request permissions. The board pairing preset now grants Administrator authority
and the exact request set supported by that board, with full-control disclosure
before approval. This is one preset, not a granular permissions editor.

No deployed users or retained-pairing migration project are in scope. Development
devices can be reset and paired again. Live capability and authorization checks
remain necessary for different boards and changing access, not for preserving
older development pairings.

## Delivery sequence

### 1. Upstream integration and full-control pairing — implemented

The app branch is rebased on upstream `8c211827b`. New board pairing uses the
Administrator/full-supported-control preset, and confirmation explains the
access being granted. The app consumes generated request and authority types;
it does not duplicate upstream authorization.

Physical acceptance must use newly paired test devices and matching firmware.
Do not infer a saved controller's authority from request bits: upstream's
resolved-target projection does not currently carry that authority.

### 2. Read-only node management — implemented

Opening a managed node reads its overview automatically and presents Interfaces,
Device, Information and, when allowed, Access sections. Per-interface settings,
discovery groups and peers are fetched on demand. Information contains
firmware/build and battery/power observations. Identity and connection diagnostics
are tucked into a disclosure; Check and Share address remain available where
allowed. Failed reads require an explicit retry, not continuous polling.

Inventory pages contain up to four entries and use typed keyset cursors. Explicit
Load more actions are bounded to 128 interfaces or peers and reject overlapping
or non-progressing pages. Pages are not an atomic snapshot: refresh resets the
traversal, and disappearing entries must not
be presented as authoritative current state. Scope reads to target, screen and
runtime generation; retain deadlines, readiness waiting and cancellation/link
cleanup already used by node checks.

Remaining physical acceptance covers differing board capabilities, zero and
more-than-four interfaces/peers, changing inventories, denied permissions,
route exit, Bluetooth loss and stale results from an earlier runtime.

### 3. Ordinary configuration — implemented

The app exposes interface power, typed LoRa profile editing, interface mode/group,
discovery-group replacement, GNSS and display actions where advertised. Rust
protocol/domain types validate changes. JavaScript checks only that numeric form
values can cross the generated bridge without truncation; it does not serialize
LoRa protocol strings or duplicate regional validation.

Use actor-owned operation IDs and observable status, as for the existing
announcement command. Serialize target changes: embedded command admission is
bounded to one queued command and Busy is a valid outcome. Distinguish Applied,
Unchanged, Scheduled, rejected, persistence failure, rollback and an unknown
outcome after a lost reply. Never automatically retry a write merely because the
screen lost its promise or connection. Leaving a screen is not an undo operation.

Several setters have no corresponding current-state getter: system awake state,
GNSS desired state, display visibility/auto-off, station uplink and ESP radio
mode. Present explicit actions or label last-requested values; do not render
invented authoritative switches. DescribePower describes battery/external power
and charging, not those configuration states. Respect advertised persistence:
some values are session-only and LoRa persistence is not universal across boards.
Saving LoRa settings can replace automatic radio selection with an explicit
profile. Inventory currently reports the resolved profile, not its original
automatic/manual mode; restoring its numeric fields does not establish that the
prior selection mode was restored.

### 4. Disruptive connectivity and power changes — implemented

Station uplink, Bluetooth/hotspot mode, radio sleep/wake and system sleep/wake
are available where supported. Inline confirmation explains the likely
disconnect and physical or alternate-route recovery before disruptive changes.
Station uplink targets its Wi-Fi interface. A sleeping/offline radio cannot
receive a wake request over that same disconnected route.

Scheduled effects have a 250 ms target response grace period, not a delivery
guarantee. Preserve uncertain outcomes until an appropriate read/reconnection
can reconcile them; do not display success just because the link disappeared.

### 5. Transactional Wi-Fi setup — implemented, qualification in progress

User flow: enter network → try connection → keep network or restore previous.
The typed Rust workflow wraps Stage, Activate, Inspect, Confirm and Cancel.
Each step is actor-owned and bounded to 30 seconds; the actor is not held during
the user's decision window. Start inspects first and refuses to overwrite a
pending trial, then stages and activates once. The screen inspects on entry and
resume, and offers revision-bound Keep/Restore actions only from a fresh result.

- Validate UTF-8 byte lengths (SSID 32, password 64), not character counts.
- Stage returns a nonzero revision. Activation is scheduled; subsequent actions
  are bound to that revision and controller identity.
- The target owns the 120-second confirmation deadline. Display an estimate,
  inspect on resume and respect the target's result. Bluetooth reachability
  alone is not proof that the new Wi-Fi network is ready for confirmation.
- Cancel requests rollback on the target; it is not local promise cancellation.
  Reboot with an unconfirmed change restores the last confirmed credentials.
- Use upstream zeroizing credential types. Never place passwords in snapshots,
  logs, analytics, persistent retry queues or operation metadata. Clear the form
  promptly without claiming JavaScript memory can be reliably zeroized.
- Only nonsecret operation metadata is retained in process. After process restart,
  inspection recovers the node's current transaction but cannot attribute a
  terminal result to a forgotten attempt. No durable trial history is claimed.
  After a lost response, inspect rather than blindly staging again. Another
  controller's transaction can be busy or inaccessible.
- Inspect does not report network readiness. Awaiting confirmation is not proof
  of successful Wi-Fi; Keep relies on the node's readiness check and can be
  refused while it is still connecting. The UI labels remaining time as the
  last observation, not a live guarantee or a locally manufactured rollback.

Acceptance requires fault injection at each transition: lost request/reply,
incorrect credentials, timeout, app restart, target reboot, concurrent controller,
persistence failure and explicit rollback. Then test the full flow physically.

### 6. Administrator access management — inventory/removal implemented

The Access section lists controller identities, labels this phone, and offers
confirmed removal of other identities. Reads use bounded pagination. Native code
rejects self-removal before dispatch; upstream also protects Administrator grants.
Refusal, already-removed and uncertain results are distinct. An accepted removal
does not optimistically erase a row; read again to observe the node. Local Forget
is not the same operation. No physical removal of another test device is required
to qualify the read-only list.

The current inventory returns identity hashes only, not roles, public identities
or per-controller permissions. Do not invent those details; richer inventory
requires a narrow upstream addition. AuthorizeController requires the full public
identity and produces an Operator grant. It does not install the reciprocal
target descriptor/access on the recipient phone. Design and validate an explicit
recipient import/pairing flow before advertising “Add another phone.”

### 7. Qualification and documentation

Each slice includes Rust workflow tests, generated-binding checks and UI tests.
Exercise the capability matrix (ESP, Nordic and custom targets),
pagination, refusal/Busy, cancellation, generation changes and lost replies.
Rebuild and test Android/Galaxy S9+ and iOS/MetalbeardMobile separately; old binary
evidence does not qualify this rebase. Run the configured firmware/resource
matrix; physical Nordic behavior remains unqualified without matching hardware.

Update the user guide, current roadmap and validation record as slices land.
Preserve dated historical checkpoints. Keep the observed board-menu freeze as a
separate unresolved issue, not an assumed consequence or fix of this expansion.

## Complete command coverage

| App surface | Upstream requests | Status |
| --- | --- | --- |
| Existing checks and address sharing | Describe, AnnounceSelf | Existing |
| Interfaces, settings and peers | InventoryInterfaces, InventoryInterfaceConfig, InventoryInterfacePeers | Implemented |
| Firmware and power information | DescribeBuild, DescribePower | Implemented |
| Interface configuration | SetInterfacePower, SetInterfaceMode, SetInterfaceGroup, SetInterfaceLoRaProfile | Implemented |
| Discovery groups | InventoryInterfaceDiscoveryGroups, ReplaceInterfaceDiscoveryGroups | Implemented |
| Positioning and display | SetGnssPower, SetDisplayVisibility, SetDisplayAutoOff | Implemented |
| Connection and power actions | SetStationUplink, SetEspRadioMode, SetSystemPower, SleepRadios, WakeRadios | Implemented |
| Wi-Fi change with rollback | StageWifiCredentials, ActivateWifiCredentials, ConfirmWifiCredentials, CancelWifiCredentials, InspectWifiTransaction | Implemented |
| Legacy Wi-Fi control | SetInterfaceWifiStation | Planned only if advertised; explain its distinct guarantees |
| Controller inventory/removal | InventoryControllers, RevokeController | Implemented |
| Grant another controller access | AuthorizeController | Planned; requires recipient onboarding |

Some framework operations, including the legacy Wi-Fi setter, interface mode and
SleepRadios/WakeRadios, are not advertised by the audited embedded targets.
Do not show controls merely because the enum exists. Implemented source support
does not establish physical support or successful operation on every board.

## Source contracts

- Controller API: `prns-runtime/impls/tokio/src/runtime/node_facade/remote_control/`.
- Protocol, authorization and pagination: `prns-core/src/remote_control/`.
- Board executor: `personal-hopspot/core/src/remote_control_executor.rs`.
- Wi-Fi persistence: `personal-hopspot/core/src/wifi_configuration_store.rs`.
- ESP capabilities: `personal-hopspot/embedded/esp32/src/{s3,c6,s3fn8}/remote_control.rs`.
- Nordic capabilities: `personal-hopspot/embedded/nrf52840/src/runtime/remote_control.rs`
  and its `headless/remote_control.rs` variant.

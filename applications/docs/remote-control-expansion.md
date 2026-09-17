# Expanded node management

Implementation plan against upstream `c4fd54dfd`, following
[PR #232](https://github.com/KenAKAFrosty/Prns/pull/232). This is a plan, not a
claim that the app already implements these controls. The current app supports
pairing, authenticated node checks and node-address sharing. The compatibility
refresh names all 28 request kinds and discloses Operator or Administrator
authority at pairing confirmation; it does not grant additional permissions.

## Ownership and product rules

Keep the existing architecture. Public PRNS owns the protocol, authorization,
board capability reporting and target-side transactions. Application Rust owns
bounded workflows and operation lifetimes. Generated UniFFI bindings carry typed
data; React Native owns presentation and platform interaction. Do not add a
generic RPC escape hatch, another transport, or a JavaScript command queue.

Show features from the live intersection of board capabilities and the paired
controller's permissions, not from board model names. Older two-operation nodes
must retain the existing useful screen. Unsupported controls are absent or
clearly unavailable, never silently simulated. Keep user-facing wording free of
protocol details. Recheck authorization and capabilities before a write.

Operator and Administrator are separate authority levels, not a request-bit
convention. Existing Operator grants remain Operator. Administrator authority permits
access management, but does not imply permission for every ordinary setting.
Our board pairing UI retains its existing Describe/AnnounceSelf Operator policy
until a deliberate permission-selection design is implemented and tested.

## Delivery sequence

### 1. Compatibility and explicit access

- Finish and qualify the new request/authority projections and generated
  bindings on both phones. Show the granted access level before approval.
- Implement and test explicit board-side permission selection before physical
  acceptance of the expanded screens. A reviewed read-only permission preset can
  unlock the inventory slice before settings and administrator enrollment are
  added. Existing pairings retain their grants; additional access requires
  explicit re-enrollment or an authorized grant change. Keep this reusable
  firmware work separate from app presentation. Provisioned test grants and UI
  fixtures do not replace a usable enrollment flow.
- Upstream currently drops authority when projecting a resolved target to the
  controller. Propose a narrow public authority projection before showing a
  persistent role badge; do not guess it from effective request bits.

### 2. Read-only node management

Add Overview, Interfaces and per-interface details/Peers, with firmware/build and
battery/power information. Keep Check and Share address available where allowed.
Fetch details on demand, not continuously in a global snapshot.

Inventory pages contain up to four entries and use typed keyset cursors. Bound
the total work and detect repeated/non-progressing cursors. Pages are not an
atomic snapshot: refresh resets the traversal, and disappearing entries must not
be presented as authoritative current state. Scope reads to target, screen and
runtime generation; retain deadlines, readiness waiting and cancellation/link
cleanup already used by node checks.

Acceptance: old minimal nodes, zero and more-than-four interfaces/peers, changing
inventories, unavailable permissions, route exit, Bluetooth loss and stale
results from an earlier runtime all behave honestly.

### 3. Ordinary configuration

Add interface power and LoRa profile editing, GNSS and display actions, plus
interface mode/group where advertised. Validate with Rust protocol/domain types;
do not maintain a separate permissive JavaScript validation model.

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

### 4. Disruptive connectivity and power changes

Add station uplink, Bluetooth/hotspot mode, radio sleep/wake and system sleep/wake
where supported. Before applying, explain the likely disconnect and the physical
or alternate-route recovery. A sleeping/offline radio cannot receive a wake
request over that same disconnected route.

Scheduled effects have a 250 ms target response grace period, not a delivery
guarantee. Preserve uncertain outcomes until an appropriate read/reconnection
can reconcile them; do not display success just because the link disappeared.

### 5. Transactional Wi-Fi setup

User flow: enter network → try connection → keep network or restore previous.
Implement a typed Rust workflow around Stage, Activate, Inspect, Confirm and
Cancel; do not expose five unrelated buttons.

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
- Retain only nonsecret workflow metadata for resume. After a lost response,
  inspect/reconcile rather than blindly staging again. Another controller's
  transaction can be busy or inaccessible.

Acceptance requires fault injection at each transition: lost request/reply,
incorrect credentials, timeout, app restart, target reboot, concurrent controller,
persistence failure and explicit rollback. Then test the full flow physically.

### 6. Administrator access management

Provide controller inventory and protected grant/revoke actions. Separate local
Forget from revoking access on the node. Confirm destructive access changes and
honor upstream restrictions: no self-revocation/modification and no modification
or revocation of an existing Administrator through these operations.

The current inventory returns identity hashes only, not roles, public identities
or per-controller permissions. Do not invent those details; richer inventory
requires a narrow upstream addition. AuthorizeController requires the full public
identity and produces an Operator grant. It does not install the reciprocal
target descriptor/access on the recipient phone. Design and validate an explicit
recipient import/pairing flow before advertising “Add another phone.”

### 7. Qualification and documentation

Each slice includes Rust workflow tests, generated-binding checks and UI tests.
Exercise the capability matrix (old minimal, ESP, Nordic and custom targets),
pagination, refusal/Busy, cancellation, generation changes and lost replies.
Rebuild and test Android/Galaxy S9+ and iOS/MetalbeardMobile separately; old binary
evidence does not qualify this rebase. Run the configured firmware/resource
matrix; physical Nordic behavior remains unqualified without matching hardware.

Update the user guide, current roadmap and validation record as slices land.
Preserve dated historical checkpoints. Keep the observed board-menu freeze as a
separate unresolved issue, not an assumed consequence or fix of this expansion.

## Complete command coverage

| App surface | Upstream requests | Slice |
| --- | --- | --- |
| Existing checks and address sharing | Describe, AnnounceSelf | Existing |
| Interfaces, settings and peers | InventoryInterfaces, InventoryInterfaceConfig, InventoryInterfacePeers | 2 |
| Firmware and power information | DescribeBuild, DescribePower | 2 |
| Interface configuration | SetInterfacePower, SetInterfaceMode, SetInterfaceGroup, SetInterfaceLoRaProfile | 3 |
| Positioning and display | SetGnssPower, SetDisplayVisibility, SetDisplayAutoOff | 3 |
| Connection and power actions | SetStationUplink, SetEspRadioMode, SetSystemPower, SleepRadios, WakeRadios | 4 |
| Wi-Fi change with rollback | StageWifiCredentials, ActivateWifiCredentials, ConfirmWifiCredentials, CancelWifiCredentials, InspectWifiTransaction | 5 |
| Legacy Wi-Fi control | SetInterfaceWifiStation | 5, only if advertised; explain its distinct guarantees |
| Controller access | InventoryControllers, AuthorizeController, RevokeController | 6 |

Some framework operations, including the legacy Wi-Fi setter, mode/group and
SleepRadios/WakeRadios, are not advertised by the audited embedded targets.
Retain typed support but do not show controls merely because the enum exists.

## Source contracts

- Controller API: `prns-runtime/impls/tokio/src/runtime/node_facade/remote_control/`.
- Protocol, authorization and pagination: `prns-core/src/remote_control/`.
- Board executor: `personal-hopspot/core/src/remote_control_executor.rs`.
- Wi-Fi persistence: `personal-hopspot/core/src/wifi_configuration_store.rs`.
- ESP capabilities: `personal-hopspot/embedded/esp32/src/{s3,c6,s3fn8}/remote_control.rs`.
- Nordic capabilities: `personal-hopspot/embedded/nrf52840/src/runtime/remote_control.rs`
  and its `headless/remote_control.rs` variant.

# Current application roadmap

This is the working backlog, not a release schedule or a claim that every shell
route works. Check [validation and limits](validation.md) before making platform
or reliability claims. Setup belongs in the [workspace guide](../README.md);
the [binding guide](../prns/native-composition/bindings/README.md) owns the current
generated API and native lifecycle boundary.

## Implemented foundation

- One application-owned Rust node and storage owner, consumed by iOS and Android
  through generated bindings with native platform admission.
- Identity creation and one-time import, local node/route inspection, contacts,
  RemoteControl pairing, authenticated checks and node-address sharing.
- Expanded node overview, interfaces, settings, peers and discovery groups,
  plus typed ordinary/disruptive changes driven by live capabilities and access.
  The new board pairing preset grants Administrator authority and the board's
  exact supported request set after full-control disclosure.
- Small direct LXMF messages, a resettable persistent mailbox, manual retry and
  local cancellation. Resources, opportunistic delivery and propagation are not
  part of this messaging implementation.
- A responsive Expo shell and stable screen catalog. Web has an explicitly
  unavailable runtime; browser/Tauri ownership decisions exist, not providers.

These are development capabilities. The [dated follow-up checkpoint](../checkpoints/2026-09-09-follow-up.md)
records completed cold Retry admission, iOS preparation-queue and connection-check
lifetime fixes, plus the constrained firmware footprint correction. Physical
observations remain tied to their recorded builds.

## Current priority: qualify read/write controls, then guided Wi-Fi

The [September 21 integration](../checkpoints/2026-09-21-remote-control-management.md)
rebases the app on upstream `8c211827b` and names all 30 RemoteControl request
kinds. The first [read/write slice](remote-control-expansion.md) is implemented:
Overview, interfaces/configuration/peers, discovery groups and 13 typed changes
cover ordinary settings and disruptive connection/power actions. Rust retains
workflow and validation ownership; generated bindings feed capability-driven UI.

Next, qualify the broader pairing preset and new controls on Galaxy S9+,
MetalbeardMobile and matching test-board firmware, then implement guided
transactional Wi-Fi setup and controller access management. Neither of those
later flows is implemented. There is no deployed-pairing migration project or
granular permissions picker: development devices can be reset and paired again.
Live upstream capability and authority checks remain mandatory.

Earlier physical and firmware evidence below belongs to its recorded
source/binaries and does not qualify this rebase or the new screens.

## Remaining qualification from the previous transition

The [September 15 checkpoint](../checkpoints/2026-09-15-upstream-integration.md)
integrates reviewed upstream trunk `35859bb89`. A new standalone Android build
and matching E290 firmware are installed. The final patched Galaxy build passes
pristine identity import, cold retention, offline Retry/Cancel, Stop/Start, three
Bluetooth cycles, two-way messaging, one controlled off-screen receipt, fresh
pairing, authenticated checks and bounded pending-read route-exit/retry recovery.
The board's navigation freeze/startup-notice event remains unresolved. Earlier
checkpoints do not qualify these binaries.

1. Diagnose the initial iOS nonconnection and recheck chooser visibility.
   The same framework later passed retained-grant checks and
   foreground messaging, including a first check after one controlled process
   restart. A later developer-SIGTERM trial captured a restoration-requested
   native relaunch, an intervening disconnect/fresh reconnect and incoming-message
   proof. The [later USB investigation](../checkpoints/2026-09-09-ios-recovery-latency.md)
   captured an acknowledged Hello without Welcome, ten-second local cleanup
   and a successful fresh handshake. The isolated correction now passes two
   bounded physical restorations without that stall, a first resumed Check and
   messaging. Extend its lifecycle qualification; it does not yet explain or fix
   the initial ordinary-start timeout.
2. Close the remaining generated-binding physical checks: iOS fresh pairing,
   wider read-cancellation cases and broader recovery. Pristine interactive
   import, fresh pairing, authenticated checks, pending-read route-exit/retry and
   three radio cycles pass on the September 15 Galaxy build; iOS import is still
   open. Diagnose the E290's roughly 15-second menu freeze followed by its startup
   notice: watchdog reset is a hypothesis, not a captured cause. The
   [offline continuation](../checkpoints/2026-09-09-offline-and-upstream-refresh.md)
   now covers cold iOS saved messages/contacts, Retry/Cancel, restart retention
   and a bounded return of the peer. Cold offline actions now also pass on the
   September 15 Android build, including same-record cancellation retention.
   Retained-grant checks and messaging have bounded evidence on
   both platforms, tied to their exact recorded builds rather than transferred
   across rebuilds. Earlier retained-grant results are not fresh-pairing acceptance.
3. Repeat the completed no-touch receipt/resume journey with a complete native
   timeline and investigate its 32.024-second submission-to-proof delay, which
   did not reproduce in the later active-traffic comparisons. One
   controlled off-screen message and first resumed Check (359 ms) passed. The
   corrected build adds a complete SIGTERM recovery timeline, incoming proof
   (485 ms) and first resumed Check (354 ms), but no separately confirmed no-touch
   window. The earlier trials remain separate. Natural suspension, quiet idle,
   Metro-off, force-quit and
   protected-data cases remain explicit qualification work. Preserve the firmware
   size gate: the September 15 integrated T-Echo S140 v7 build leaves just
   **328 bytes** of FLASH headroom. Only T-Echo and E290 were remeasured in this
   pass; the full configured profile matrix remains separate. Remeasure after
   firmware-relevant changes.
4. Finish upstream integration and landing. September 12 publication opened
   eleven draft PRs, [#215](https://github.com/KenAKAFrosty/Prns/pull/215) through
   [#225](https://github.com/KenAKAFrosty/Prns/pull/225), and updated the shared
   Host snapshot [#201](https://github.com/KenAKAFrosty/Prns/pull/201). This includes
   combined Nordic diagnostics/footprint (#215), native iOS recovery
   [#218](https://github.com/KenAKAFrosty/Prns/pull/218), and atomic CoreBluetooth
   write batches [#223](https://github.com/KenAKAFrosty/Prns/pull/223); they are no
   longer unpublished candidates. Keep the Nordic changes together: the recorded
   diagnostics-only parent does not fit T-Echo. Physical write batching remains
   unqualified. Recheck upstream overlap, PR dependencies and remote CI before
   rebasing affected branches; passing local publication gates did not make the
   entire remote matrix green. The [validation record](validation.md#firmware-and-repository-checks)
   separates these results from app qualification and device evidence. Use
   [#197](https://github.com/KenAKAFrosty/Prns/pull/197) for the app's current
   publication and remote CI status.

Carry these qualification gaps alongside the new work; do not treat either
source rebasing or new UI coverage as device acceptance.

## Following product slices

| Slice | Outcome and boundary |
| --- | --- |
| Direct LXMF Resources | Send/receive messages above the Link-packet limit using the existing wire and mailbox ownership; prove Python interoperability, bounds and cancellation. This is the next messaging increment, not propagation. |
| Complete everyday controls | Message detail, pairing forget/revocation and further node operations, plus local interface/identity management where public APIs exist. Add missing generic seams upstream; do not duplicate protocol authority in the app. |
| Notifications | Explicit privacy/settings and platform posting policy, with lifecycle evidence for each supported behavior. A running-node notification is not a message alert or a delivery guarantee. |
| Later LXMF modes | Identified-Link reuse, ratchets/opportunistic delivery, stamps/tickets and propagation as separate protocol/state slices with reference tests. |
| NomadNet client | A bounded client-only request/cache model and non-executing Micron rendering, with a reference corpus and malformed-content limits. Hosting and dynamic actions remain separate. |
| Location and inspection | Explicit per-send consent and typed location provenance; richer activity, routes, interfaces and diagnostics. Location permission alone must not sample or transmit. |

Choose one slice at a time. Define its observable outcome, public Prns inputs,
owned modules, generated changes and focused acceptance before implementation.

## Deferred platform and distribution work

Browser needs a reusable public DedicatedWorker orchestration seam; desktop
needs a direct Tauri composition with process and shared-instance ownership.
The [dated architecture checkpoint](../checkpoints/browser-tauri/README.md)
records those decisions and must be re-audited against upstream before either
provider is built. Do not create a second protocol engine or copy its scheduler.

Newer Android service/permission behavior, iOS restoration, release/R8/signing,
accessibility/localization, secret custody, retained-data upgrades and broader
transport coverage remain explicit qualification work. Identity export,
recovery, concurrent identity use and multi-device mailbox synchronization are
separate security/protocol projects; onboarding import does not provide them.

## Development rules worth preserving

- Keep dependencies one-way: `applications/` consumes public Prns; core does not
  import application services or product policy.
- Rust owns protocol behavior, identity, durable state and command lifetime.
  TypeScript owns presentation and platform-facing orchestration; generated
  bindings are outputs, not another domain model.
- Current development data is disposable. Persistence supports real workflows,
  but is not a secure-storage, backup or cross-version migration guarantee.
- Preserve the [screen IDs](../prns/app/src/navigation/catalog.ts), update route
  parameters and deep-link tests together, and distinguish real capability from
  placeholder UI. Avoid board-specific and implementation-detail user copy.
- Refactor for demonstrated reuse. Do not add a service registry, storage
  abstraction or platform package solely to anticipate a later feature.

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
- Small direct LXMF messages, a resettable persistent mailbox, manual retry and
  local cancellation. Resources, opportunistic delivery and propagation are not
  part of this messaging implementation.
- A responsive Expo shell and stable screen catalog. Web has an explicitly
  unavailable runtime; browser/Tauri ownership decisions exist, not providers.

These are development capabilities. The [follow-up checkpoint](../checkpoints/2026-09-09-follow-up.md)
records completed cold Retry admission, iOS preparation-queue and connection-check
lifetime fixes, plus the constrained firmware footprint correction. Physical
observations remain tied to their recorded builds.

## Next: finish the current transition

1. Diagnose the initial iOS nonconnection and recheck chooser visibility.
   The same framework later passed retained-grant checks and
   foreground messaging, including a first check after one controlled process
   restart. A later developer-SIGTERM trial captured a restoration-requested
   native relaunch, an intervening disconnect/fresh reconnect and incoming-message
   proof. The [later USB investigation](../checkpoints/2026-09-09-ios-recovery-latency.md)
   captured an acknowledged Hello without Welcome, ten-second local cleanup
   and a successful fresh handshake. Finish and qualify the isolated restored-
   session correction; it does not yet explain or fix the initial ordinary-start
   timeout.
2. Close the remaining generated-binding physical checks: fresh pairing,
   pristine import, controlled read cancellation, repeated recovery and cold offline
   actions. Retained-grant checks and messaging now have bounded evidence on
   both platforms, tied to their exact recorded builds rather than transferred
   across rebuilds. They are not fresh-pairing acceptance.
3. Repeat the completed no-touch receipt/resume journey with a complete native
   timeline and investigate its 32.024-second submission-to-proof delay, which
   did not reproduce in the later active-traffic comparisons. One
   controlled off-screen message and first resumed Check (359 ms) passed; the
   earlier logged restoration trial remains separate because of possible user
   activity. Natural suspension, quiet idle, Metro-off, force-quit and
   protected-data cases remain explicit qualification work. Preserve the firmware
   size gate: the passing integrated T-Echo build still has just 616 bytes spare.

Finish this bounded transition before expanding the product surface.

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

# Current application roadmap

This is the working backlog, not a release schedule or a claim that every shell
route works. Check [validation and limits](validation.md) before making platform
or reliability claims. Setup belongs in the [workspace guide](../README.md);
the [binding guide](../prns/native-composition/bindings/README.md) owns the current
generated API and native lifecycle boundary.

## Implemented foundation

- App-owned Rust composition and storage above the shared native host and
  general Expo SDK. Host bindings and generic platform mechanics belong to
  the SDK; the app stages that package with its one aggregate native image.
- Identity creation and one-time import, local node/route inspection, contacts,
  RemoteControl pairing, authenticated checks and node-address sharing.
- Expanded node overview, interfaces, settings, peers and discovery groups,
  plus typed ordinary/disruptive changes driven by live capabilities and access.
  The new board pairing preset grants Administrator authority and the board's
  exact supported request set after full-control disclosure.
- Guided Wi-Fi trials with explicit keep/restore and uncertain-outcome recovery,
  plus controller inventory and protected access removal. Adding a controller
  still requires recipient identity exchange and reciprocal setup.
- Small direct LXMF messages, a resettable persistent mailbox, manual retry and
  local cancellation. Resources, opportunistic delivery and propagation are not
  part of this messaging implementation.
- A responsive Expo shell and stable screen catalog. Web has an explicitly
  unavailable runtime; browser/Tauri ownership decisions exist, not providers.

These are development capabilities. The
[SDK adoption checkpoint](../checkpoints/2026-09-24-sdk-adoption.md) records bounded
Android/iOS ownership and Hermes reload checks, final standalone Release cold
starts and retained app data. The
[standalone SDK qualification](../../prns-react-native/docs/qualification.md)
separately records the default provider. Physical observations belong to their
recorded builds; neither record qualifies every background or product workflow.

## Current priority: usable on-phone nodes and a two-phone demo

The [two-phone demo plan](phone-node-demo.md) brings local-node,
Contacts/Discovered and interface-management requirements into one observable
journey: connect iOS and Android, announce distinct messaging names, save
contacts, exchange LXMF messages over automatic, pair-free BLE, and inspect how
the network is communicating. The
[ordinary CoreBluetooth checkpoint](../checkpoints/2026-09-23-ordinary-corebluetooth.md)
records direct discovery, two-way messages and one bounded off-screen receipt.
The remaining UX and inspection slices are still planned.

Finish truthful connection states, then announcing/discovery/contact UX and
bounded network inspection. TCP remains outside this product milestone. Preserve
native background ownership and qualify foreground, locked/background and
recovery behavior separately. The app uses ordinary Bluetooth permission and
dual-role CoreBluetooth; the old per-accessory chooser investigation is retired.
Keep app-level Bluetooth permission, messaging discovery and RemoteControl
board pairing distinct.

This prioritization does not make more board-management features prerequisites
for using the phone's own node. Direct LXMF Resources follows this usability
slice rather than blocking the first short-message demo.

## Parallel backlog: remote settings qualification and recipient onboarding

The [September 21 integration](../checkpoints/2026-09-21-remote-control-management.md)
rebases the app on upstream `8c211827b` and names all 30 RemoteControl request
kinds. The first [read/write slice](remote-control-expansion.md) is implemented:
Overview, interfaces/configuration/peers, discovery groups and 13 typed changes
cover ordinary settings and disruptive connection/power actions. Rust retains
workflow and validation ownership; generated bindings feed capability-driven UI.

The [settings workflow checkpoint](../checkpoints/2026-09-21-remote-settings-workflows.md)
adds automatic reads, organized sections, transactional Wi-Fi trials and access
inventory/removal. Recorded Android/board checks cover LoRa read/write and
restored values after restart, plus failed Wi-Fi trials and prompt explicit
rollback. Next, qualify successful new-network Keep and extend these checks to
iOS. Design verified recipient onboarding before exposing controller
authorization. There is no deployed-pairing migration project or granular
permissions picker: development devices can be reset and paired again. Live
upstream capability and authority checks remain mandatory.

## Qualification and integration work to carry forward

- **Current mobile lifecycle:** permission and radio recovery, natural
  suspension, long/locked idle, repeated restoration, protected-data access and
  first-request recovery still need bounded, recorded journeys on current builds.
  Normal standalone cold startup and retained-data upgrades have passed on iOS
  and Android; they are not background-delivery guarantees.
- **Product acceptance:** qualify fresh iOS identity import and pairing, wider
  cancellation/disconnection cases, and the remote-settings workflows above.
  Earlier Android and iOS journeys remain in their dated checkpoints; retained
  grants do not prove fresh pairing.
- **Unresolved historical failures:** retain the board navigation freeze/startup
  notice, the old iOS ordinary-start timeout and the unexplained long delivery
  delay as investigation leads. The
  [September 15 checkpoint](../checkpoints/2026-09-15-upstream-integration.md) and
  [iOS recovery investigation](../checkpoints/2026-09-09-ios-recovery-latency.md)
  preserve their source, symptoms and narrower successful corrections. Reproduce
  on a current build before prescribing another fix; chooser behavior is no
  longer relevant to the current permission model.
- **SDK and app landing:** land the independent
  [SDK PR](https://github.com/KenAKAFrosty/Prns/pull/251) before the
  [app PR](https://github.com/KenAKAFrosty/Prns/pull/197). The separate inherited
  firmware-assurance and tester-roster repair has passed locally with a fresh
  clean-commit baseline; land that repair as recorded in the
  [build cleanup](../checkpoints/2026-09-24-build-cleanup.md#separate-upstream-ci-repair).
  Consult current PR checks for CI status; historical publishing hooks do not
  establish current hosted success.
- **Recorded release:** after the SDK lands, promote the app compatibility
  revision and matching artifacts, then run the separate recorded-release gate.
  Keep current-source detached receipts distinct from release qualification.
- **Firmware and transport limits:** preserve the firmware resource gates and
  remeasure firmware-relevant changes. Broader physical write-batching and board
  coverage remain separate from app builds; dated measurements are in
  [validation](validation.md#firmware-and-repository-checks).

The [validation guide](validation.md) links prior evidence and its limits. Older
PR publication chronologies belong in their checkpoints, not the active backlog.

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

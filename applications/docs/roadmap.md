# Current application roadmap

This is the working backlog, not a release schedule. The
[October 1 product scope](product-scope.md) makes direct and group messaging the
primary product, backed by a reusable messaging library. The next design work is
the conversation/group model, considered together with multi-device identity,
state synchronization and application-protocol choices. No implementation of
those new capabilities is implied by this plan.

Use the [product scope](product-scope.md) for agreed goals and open decisions,
the [workspace guide](../README.md) for setup and ownership, the
[app guide](../prns/app/README.md) for behavior and routes, and the
[validation index](validation.md) for exact evidence and limits. Completed plans
and dated receipts are history, not additional current authorities.

## Milestones

| Milestone | Implementation | Recorded device acceptance | Remaining work | Landing dependencies |
| --- | --- | --- | --- | --- |
| Foundation and persistence | Native composition, identities/import, contacts, mailbox and generated bindings | Retained-data installs and interval saves on both phones; Android import | Fresh current-build iOS import; custody and distribution are not promised | App [#197](https://github.com/KenAKAFrosty/Prns/pull/197); sandbox repair [#256](https://github.com/KenAKAFrosty/Prns/pull/256) is already integrated |
| Reusable Expo SDK | Public host/session mechanics below app services; one selected native image | Default SDK and app-owned aggregate have separate ownership/reload evidence | Detached aggregate iOS consumer, public distribution and recorded release | SDK [#251](https://github.com/KenAKAFrosty/Prns/pull/251) before app; includes #256 |
| Two-phone BLE messaging | Pair-free automatic BLE, announce/discover/save, short direct messages, retry/cancel, paged history and restart recovery | Saved-contact delivery and isolated restarts in both directions; bounded off-screen receipts | Device acceptance of mailbox review fixes; churn, silent-peer expiry and long idle remain reliability work | Recovery [#257](https://github.com/KenAKAFrosty/Prns/pull/257) follows #251 and is already integrated into #197 |
| Network inspection | Physical connections, current routes and session announcement history | Focused standalone journey on both phones, including clear/reannounce and two-way messaging | Conversation delivery details; never infer past message paths from current routes | App #197 on the SDK/recovery integration |
| Remote management | Capability-driven read/write screens, Wi-Fi trial workflow and access inventory/removal | Bounded Galaxy/E290 reads/writes, restart retention and failed-trial rollback | Successful Wi-Fi Keep, iOS coverage and verified recipient onboarding | App #197 consumes upstream RemoteControl APIs |
| Reusable direct and group messaging | Existing direct-LXMF service and wire crates are a starting point, not the complete general library | Existing direct-message evidence only; no group acceptance | Agree domain/protocol design, remove product coupling and prove an independent consumer | Public Prns host APIs; distinct from the Expo host SDK |
| Multiple devices and synchronization | One local identity/mailbox and one-time identity import today; no linked-device protocol | No coordinated multi-device acceptance | Joint identity, group, delivery and sync design; choose reusable mechanisms versus app policy | Resolve relevant constraints before freezing messaging schemas |
| Optional supporting scope | A local messaging name exists; other shell routes are not features | No social/profile-system acceptance | Evaluate light profiles/social, richer messages and notifications against messaging needs | No mandatory NomadNet, LXSF or complete LXMF feature catalog |

The [validation index](validation.md) links the source and binaries for each
journey. Retained grants do not prove fresh pairing; development-signed builds
and current-source detached checks do not qualify a recorded release.

## Next product increment

The sequence below is the proposed implementation path for the agreed scope.
It does not preselect a wire format, key model or synchronization engine.

1. **Define direct and group user journeys and the domain contract.** Specify
   conversation identity, participant/device distinctions, group type and
   membership authority, offline behavior, verification and delivery evidence.
   Keep routine actions simple and full supported controls/details reachable.
2. **Review groups, identity and sync together.** Compare multiple devices under
   one identity, enrollment/revocation, history and human state conflicts.
   Evaluate LXMF compatibility against those outcomes and the desired core
   Reticulum compatibility. Record selected tradeoffs and the first increment's
   explicit limits before committing to schemas or cryptographic mechanisms.
3. **Establish the reusable messaging contract.** Evolve or compose the existing
   Rust services as the reviewed design requires; remove product storage/API
   assumptions. Prove an independent headless consumer with its own composition
   and storage ownership. This boundary is required even if the first consumer
   remains the Prns app. Do not create a generic service registry or speculative
   multi-protocol framework.
4. **Deliver a bounded direct/group messaging journey.** A proposed initial
   acceptance setup is the two phones plus an independent headless participant.
   Exercise the selected membership model, direct and group send/reply, partial
   delivery, offline/restart recovery, duplicate handling and unauthorized
   actions. Device linking/sync is included only to the extent explicitly chosen
   by step 2, not inferred from importing the same key twice.
5. **Refine everyday use and advanced inspection.** Prioritize discovery,
   invitations, conversation navigation, composition and useful failure recovery.
   Add delivery details from recorded evidence, then select richer content,
   notifications or light profiles based on the messaging journey.

In parallel, retain the working direct-message baseline and qualify the
[mailbox review fixes](../checkpoints/2026-09-30-mailbox-review.md) on matching
phone builds. Their automated evidence is separate from the earlier phone
builds. Existing compatibility tests remain active for supported LXMF behavior;
a scope revision does not justify deleting them.

The [two-phone demo](phone-node-demo.md) remains a bounded baseline: open the apps,
announce distinct messaging names, discover and save contacts, exchange messages
over ordinary BLE, and inspect the communication. It is not the complete product
goal or proof of group/multi-device support. Network now separates
connections, routes and the latest 200 accepted announcements; clearing history
preserves contacts, routes and messages. The
[September 30 Network checkpoint](../checkpoints/2026-09-30-network-inspection.md)
records the focused phone journey. TCP remains outside that BLE acceptance
slice, not prohibited by the general messaging-library scope.

Ordinary Bluetooth permission, messaging discovery and RemoteControl board
authorization are distinct flows. The app uses dual-role CoreBluetooth, not the
retired accessory chooser. It must work as a phone-based node without a board.

## Separate reliability and remote management follow ups

The [shared Bluetooth recovery design](../../docs/bluetooth-session-recovery.md)
and [September 30 trials](../checkpoints/2026-09-30-mobile-liveness.md) establish
bounded restart recovery, not uninterrupted connectivity or dependable background
delivery. Follow up on competing-connection churn, unexplained status-8
disconnects, silent-peer expiry, cooperative shutdown and longer unplugged idle.
Natural suspension, repeated restoration, protected-data access and the remaining
permission/radio cases need their own recorded journeys. These are not gates
for unrelated foreground features unless a new failure blocks normal use.

The [remote-control plan](remote-control-expansion.md) and
[settings checkpoint](../checkpoints/2026-09-21-remote-settings-workflows.md)
separate implemented controls from acceptance. Qualify successful new-network
Keep and iOS workflows, then design verified recipient exchange before exposing
controller authorization. Pairing uses one disclosed full-control preset, not
a granular permissions picker. Live capability and authority checks remain
mandatory; disposable test data does not require a deployed-pairing migration.

Keep the historical board navigation freeze/startup notice, ordinary iOS startup
timeout and long delivery delay as investigation leads, not proven current
failures. Their [integration](../checkpoints/2026-09-15-upstream-integration.md)
and [recovery](../checkpoints/2026-09-09-ios-recovery-latency.md) records preserve
the limits. Reproduce on a relevant build before prescribing another fix.

## Upstream landing and release

The recommended review order is the independent sandbox repair #256, SDK #251,
Bluetooth recovery #257, then app #197. The SDK includes #256, #257 includes the
SDK work, and the app includes both; #257 is a follow-up dependency even though
its GitHub base is currently `trunk`, not the SDK branch. This is the intended
landing order, not a claim that GitHub has a true stacked base or that checks
have passed. Reconcile each remaining diff as dependencies land and consult
the PRs for live status.

Upstream already contains the inherited firmware-assurance and tester-roster
repairs recorded in the [build cleanup](../checkpoints/2026-09-24-build-cleanup.md#separate-upstream-ci-repair).
Do not reopen those historical repair candidates.

After the SDK lands, promote the app compatibility revision and matching
artifacts, then run recorded-release qualification. Current-source detached
checks remain distinct. Remeasure firmware-relevant changes with the canonical
resource gates; historical headroom is not a current firmware assurance claim.

## Following product slices

| Slice | Outcome and boundary |
| --- | --- |
| Richer messages and attachments | Choose payload, storage and transfer behavior after the messaging/protocol decision. LXMF Resources is a candidate where compatible; not an unconditional next implementation. |
| Everyday and advanced controls | Retain node/network management and expose full supported controls progressively. Further operations should support communication or a concrete advanced need; missing general APIs belong upstream. |
| Message notifications | Explicit privacy/settings and posting policy with platform evidence. A running-node notification is not a message alert or delivery guarantee. |
| Application protocol interoperability | LXMF remains desirable, not mandatory for every future flow. Select later modes or extensions from the reviewed messaging needs; identify compatibility limits explicitly. |
| Light profiles and social features | Optional aids to people/group discovery and conversation, with identity binding, visibility and abuse controls. No required NomadNet implementation or committed social feed. |
| Location and broader browsing | Optional future scope, not messaging prerequisites. Location requires explicit consent/provenance; a placeholder or historical NomadNet plan is not a delivery commitment. |
| Browser and desktop | DedicatedWorker orchestration for browser; process/shared-instance ownership for Tauri. Re-audit the [architecture checkpoint](../checkpoints/browser-tauri/README.md); do not duplicate the protocol engine. |

Choose one slice at a time. Define its observable outcome, public Prns inputs,
owned modules, generated changes and focused acceptance before implementation.
Identity export/recovery, concurrent identity use and synchronization require
their own security/protocol decisions within the joint messaging design review;
onboarding import does not provide them. Keep unknowns explicit rather than
silently carrying forward single-device assumptions.

## Development rules

- Dependencies remain one-way: `applications/` consumes public Prns APIs;
  core does not import application services or product policy.
- Rust owns protocol behavior, identity, durable state and command lifetime.
  TypeScript owns presentation and platform orchestration; generated bindings
  are outputs, not another domain model.
- Reusable messaging owns messaging correctness independently of the app.
  Core/host SDK mechanics, the messaging library and product composition are
  distinct boundaries. Reusability does not require moving product policy into
  core or implementing every possible adapter now.
- Development data is disposable, but persistence must support the workflow
  being tested. This is not a secure-storage, backup or migration guarantee.
- Preserve [screen IDs](../prns/app/src/navigation/catalog.ts); update parameters,
  links and deep-link tests together. Distinguish capability from placeholders.
  Use consumer-facing copy without board-specific or implementation jargon.
- Preserve useful existing code while meeting the required library boundary.
  Validate it with an independent consumer; use concrete requirements to choose
  further abstractions rather than creating a universal framework.

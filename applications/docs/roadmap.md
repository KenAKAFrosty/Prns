# Current application roadmap

This is the working backlog, not a release schedule. The app has a demonstrated
two-phone BLE messaging journey and usable node-management screens. The next
product increment is conversation delivery detail; wider background reliability
and remote-settings acceptance remain separate qualification work.

Use the [workspace guide](../README.md) for setup and ownership, the
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
| Later product scope | Shell routes or plans only where not explicitly implemented | No acceptance implied by placeholder routes | Resources, message notifications, location, NomadNet, later LXMF modes and browser/Tauri providers | Define each bounded slice before implementation |

The [validation index](validation.md) links the source and binaries for each
journey. Retained grants do not prove fresh pairing; development-signed builds
and current-source detached checks do not qualify a recorded release.

## Next product increment

The [review fixes](../checkpoints/2026-09-30-mailbox-review.md) add
conversation-level paging, guarded refreshes and preview verification warnings.
Their automated evidence is separate from the earlier phone builds. Next, add
conversation delivery details: show the message's recorded state, sender
verification and available delivery evidence without guessing its network path.
Use the existing Rust-owned service records and generated bindings. Do not add
a generic service registry or a new storage framework for this slice.

The [two-phone demo](phone-node-demo.md) remains the product goal: open the apps,
announce distinct messaging names, discover and save contacts, exchange messages
over ordinary BLE, and inspect the communication. Network now separates
connections, routes and the latest 200 accepted announcements; clearing history
preserves contacts, routes and messages. The
[September 30 Network checkpoint](../checkpoints/2026-09-30-network-inspection.md)
records the focused phone journey. TCP remains outside this milestone.

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
| Direct LXMF Resources | Messages above the Link-packet limit using existing wire/mailbox ownership; reference interoperability, bounds and cancellation. Not propagation. |
| Everyday controls | Pairing forget/revocation and further node/interface/identity operations where public APIs exist. Missing general APIs belong upstream. |
| Message notifications | Explicit privacy/settings and posting policy with platform evidence. A running-node notification is not a message alert or delivery guarantee. |
| Later LXMF modes | Identified-Link reuse, ratchets/opportunistic delivery, stamps/tickets and propagation as separate protocol/state slices. |
| NomadNet client | Bounded client-only requests/cache and non-executing Micron rendering. Hosting and dynamic actions remain separate. |
| Location | Explicit per-send consent and typed provenance. Permission alone must not sample or transmit. |
| Browser and desktop | DedicatedWorker orchestration for browser; process/shared-instance ownership for Tauri. Re-audit the [architecture checkpoint](../checkpoints/browser-tauri/README.md); do not duplicate the protocol engine. |

Choose one slice at a time. Define its observable outcome, public Prns inputs,
owned modules, generated changes and focused acceptance before implementation.
Identity export/recovery, concurrent identity use and multi-device mailbox
synchronization are separate security/protocol projects; onboarding import does
not provide them.

## Development rules

- Dependencies remain one-way: `applications/` consumes public Prns APIs;
  core does not import application services or product policy.
- Rust owns protocol behavior, identity, durable state and command lifetime.
  TypeScript owns presentation and platform orchestration; generated bindings
  are outputs, not another domain model.
- Development data is disposable, but persistence must support the workflow
  being tested. This is not a secure-storage, backup or migration guarantee.
- Preserve [screen IDs](../prns/app/src/navigation/catalog.ts); update parameters,
  links and deep-link tests together. Distinguish capability from placeholders.
  Use consumer-facing copy without board-specific or implementation jargon.
- Refactor for demonstrated reuse. Do not create a registry, storage abstraction
  or platform package solely for a possible later consumer.

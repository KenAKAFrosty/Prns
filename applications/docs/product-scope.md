# Messaging product scope and design questions

The October 1, 2026 maintainer agreement makes this primarily a messaging and
group messaging app for people who currently use Meshtastic or MeshCore. The
product should make everyday communication easy while exposing full control
and accurate technical detail when the user wants it. It is not primarily a
general node-administration console or a collection of Reticulum protocol demos.

This document owns the agreed scope and unresolved architectural questions.
The [roadmap](roadmap.md) owns sequencing; [validation](validation.md) owns
implementation and test evidence. This scope revision changes neither the
running app nor its protocols. It supersedes conflicting priorities in the
original scratch plans without rewriting their historical evidence.

## Agreed direction

- Direct and group messaging are primary product capabilities. The intended
  audience guides usability; it does not imply Meshtastic or MeshCore wire,
  radio-network or account interoperability.
- Simple defaults and progressive disclosure must coexist with full supported
  controls and inspectable detail. Advanced users should not lose capabilities
  merely to keep the first screen simple.
- Messaging functionality must be a reusable Rust crate/library that another
  application can compose over Reticulum without depending on this app.
- LXMF interoperability is desirable, not an unconditional requirement. Keep
  the implemented LXMF path while evaluating whether it meets the new goals.
- Compatibility with core upstream Reticulum remains the desired network
  boundary. Divergence at application layers, including LXMF, NomadNet or LXSF,
  is acceptable when a documented design tradeoff delivers a better experience.
- Multiple devices tied to one identity and synchronization between them are
  central open design questions, not solved features or a selected key model.
  How much can be generalized belongs in that investigation.
- Lightweight profiles or social features are possible supporting scope. They
  are not yet committed features and need not use NomadNet.

The phone must remain useful without an external board. Existing node management
and network inspection support communication and advanced control; they are not
being removed. A managed board is not automatically another messaging device
for the same person, and RemoteControl authorization is not messaging membership.

## Product experience

The primary journey is finding a person or group, joining or starting a
conversation, and understanding what happened to a message. Identity, addressing,
interface configuration and delivery mechanics should not dominate ordinary
conversation screens. Offline, queued, failed and partially delivered messages
still need clear states and useful recovery actions.

Keep sender authentication, device receipt, transport proof and human read state
distinct. For a group or linked devices, one successful delivery must not imply
that every participant or device received or read the message. Technical detail
should explain the recorded evidence, not reconstruct a past path from today's
route. Powerful controls remain reachable, with target, authority and consequence
visible before an action. Ease of use does not waive security or data-loss warnings.

This is a UX direction, not approval of a new navigation graph. Existing routes
remain intact until a messaging-first interaction pass deliberately revises the
catalog, entry points and deep-link tests together.

## Required reusable messaging boundary

The messaging library sits above public Reticulum/Prns APIs and below the app.
It is distinct from the general [Expo host SDK](react-native-sdk-implementation.md).
Reusability is required from this point forward, not conditional on a later
decision to extract the app. A separate consumer will validate that boundary.

| Layer | Intended responsibility |
| --- | --- |
| Reticulum and Prns host/platform SDKs | Network/runtime access, transport, platform lifecycle and generic host mechanics; no product or messaging-domain dependency |
| Reusable messaging library | Direct/group conversation semantics, authenticated message and membership processing, delivery evidence, deduplication, persistence contracts and bounded command/event/query behavior |
| Protocol and storage integrations | The chosen application protocol, any explicitly supported LXMF compatibility, and concrete storage/host adapters with declared capabilities |
| App composition and UI | Installation setup, service composition, navigation, presentation, permissions, notification presentation and explicit product policy |

Do not put group membership or message authentication only in React screens or
the app aggregate. Correctness of selected reusable messaging/device-sync
mechanisms belongs with those mechanisms; application-specific state sync and
merge policy may remain in app-owned modules. The device/sync review must decide
that boundary rather than assuming all synchronization is generic. Do not move
app routes, board-management workflows, branding or OS permission prompts into
the library. Callers must control product choices such as retention, presentation
and notification behavior through the supported contract.

The existing [LXMF wire crate](../services/lxmf-wire/src/lib.rs) and
[delivery service](../services/lxmf/src/direct.rs) are useful reusable foundations,
not the completed general messaging library. The current
[mailbox](../services/lxmf/src/mailbox.rs) models direct conversations by peer,
stores LXMF-specific records and shares an application metadata table. Its
caller-owned storage submission and borrowed host access are useful boundaries
to preserve; schema/ownership coupling needs review before the reusable contract
is called complete. The current [messaging profile](../prns/native-composition/src/messaging_profile.rs)
is a display name and messaging address, not a social-profile service.

Decide whether to evolve the existing service or compose it beneath a broader
library after the domain and protocol review. Do not select a crate name, move
packages, build a multi-protocol framework or promise every runtime backend in
this documentation pass. Repository location and publication are separate from
the mandatory dependency boundary.

Acceptance must include an independently composed consumer, initially a small
headless Rust client or harness, that exercises the selected direct and group
flows without app/Expo imports or product database assumptions. Document the
supported host, storage and runtime requirements. Reusable does not mean every
backend or embedded target is already supported.

## Protocol decision

Compare the existing LXMF path, an explicitly defined compatible extension, and
a distinct application protocol only against concrete user and security needs.
No finding has yet established that LXMF cannot meet those needs, and this scope
does not authorize an immediate replacement.

Evaluate direct/group addressing, authentication and membership changes,
multi-device delivery, intermittent connectivity, payload/fan-out cost, history
and receipt semantics, implementation complexity and interoperability. Describe
exactly which journeys work with ordinary LXMF peers and which would require
participating clients. Core Reticulum compatibility and application-protocol
compatibility are separate claims with separate tests.

Any selected divergence needs a written rationale, clear protocol identification
and version/capability handling, unsupported-peer behavior, a threat model and
reference tests. Never silently label a non-LXMF exchange LXMF-compatible or
downgrade group security to reach an older peer. Keep existing conformance checks
for the LXMF behavior we still advertise. Missing generic host capabilities
should be proposed separately; product policy must not flow into core.

## Groups to define before implementation

Group messaging is committed scope; its model is not yet selected. Decide:

- Whether the first experience is private invited groups, discoverable/public
  channels, or both, and how a conversation differs from a transport group.
- Who may invite, join, remove members or change roles; how membership is
  authenticated; and what removal means for future messages and past history.
- Encryption/key lifecycle, offline membership changes, replay protection,
  ordering, duplicate suppression and behavior during network partitions.
- How many people/devices a group can support within explicit resource budgets,
  how distribution/fan-out works, and what partial delivery looks like.
- Whether history is shared with new members and how blocking, abuse controls,
  retention and optional read receipts interact with the group model.

Do not equate a Reticulum destination, a managed board, a person and a group.
The messaging model must distinguish participants, devices, destinations and
conversations without requiring board-management concepts or forcing every
consuming app to adopt this product's screens or social model.

## Multiple devices and state synchronization

First define what users mean by one identity: a stable person-facing identity,
a Reticulum identity/destination, a signing credential or a linked device set.
These may be related without being identical. The existing one-time private-key
import is provisioning, not device enrollment or synchronization. Copying the
same key does not by itself define routing, delivery, revocation or shared state.

Consider these as alternatives, not decisions:

| Model | Questions to resolve |
| --- | --- |
| Shared private identity on several devices | Concurrent destination ownership, sending/receiving, compromise scope, key-state coordination and device removal |
| Stable logical identity authorizing separate device credentials | Enrollment, device-list authenticity/privacy, addressing, fan-out, revocation and compatibility with unmodified peers |
| A designated receiving device or mailbox service | Availability, offline collection, trust and encryption boundaries, recovery, optional versus required infrastructure |

These models can combine, but forwarding, storage and authority must be specified
separately. A relay need not be assumed trusted with plaintext, nor can that
property be claimed before an encryption design exists. Removing a device cannot
erase plaintext or keys it already obtained.

The design must distinguish the state being synchronized:

| State | Decision needed |
| --- | --- |
| Messages, attachments and authentication evidence | Stable identity, provenance, bounded transfer, duplicates, retained history and gap recovery |
| Device and group membership | Authority, enrollment/removal, key changes and reconciliation after an offline interval |
| Delivery evidence | Any-device versus every-device delivery, per-device/per-member receipts and the meaning of a user-visible success |
| Read state, drafts, deletion and archive state | Local versus shared scope, conflicting offline edits, deletion/retention semantics and recovery |
| Contact aliases, profile data and preferences | Private versus published data, authorization, merge rules and per-app choices |

Require explicit outcomes for concurrent sends, long disconnection, device loss,
new-device history access and revocation. Decide whether all devices must work
independently or whether a primary device is acceptable. Investigate reusable
mechanisms such as authenticated records, resumable cursors and idempotent
operations separately from use-case choices such as read-state merge policy.
Do not assume a universal sync engine, shared database replication, wall-clock
last-write-wins or synchronized private keys as the answer.

This research starts before locking in group and message schemas; it need not
imply that a complete multi-device product ships in the first increment. The
review must explicitly choose what the first increment supports and what remains
unsupported rather than leaving accidental constraints in the API.

## Optional scope and next decisions

Profiles/social should be evaluated as aids to conversation and group discovery,
with explicit visibility, identity binding, consent, retention and abuse controls.
An editable messaging name already exists; a social graph, public feed, presence
service or NomadNet client does not follow automatically from this goal.

The next planning deliverable is a reviewed direct/group user journey and domain
contract, with a joint identity/device/sync threat-model comparison and protocol
tradeoff record. Then select a bounded implementation slice and its library,
independent-consumer and phone acceptance tests. Larger messages/attachments,
notifications and detailed delivery inspection should follow the messaging
priorities and chosen model; implementing every LXMF mode, NomadNet, LXSF,
location or a new desktop/browser provider is not a prerequisite.

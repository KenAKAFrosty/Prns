# PRNS chat product scope

The app provides messaging, named identities, node management and network
inspection. A phone can communicate without an external board. Routine messaging
and technical controls share a host but retain separate identities and authority.

The [source walkthrough](../README.md) maps the application, the
[capability overview](../../docs/composable-features.md) explains reusable owners,
and [validation](validation.md) defines repeatable checks and their limits.

## Current features

| Area | Behavior |
| --- | --- |
| Named identities | Separate local identities, contacts and conversations on one shared node; activation, pause/resume and cross-identity activity |
| Contacts | Saved contacts with private labels and pinned-first ordering; separate Add contact and My card screens for reviewed signed links and QR exchange, bound to an explicit identity |
| Discovery | Find compatible signed contact announcements, review new cards, open existing saved contacts, and announce the selected identity through its shared native schedule |
| Conversations | Create groups, exchange authenticated text and retain local history; open exact message details and device confirmations; read current connection activity separately |
| Invitations | Search saved contacts or review an identity address, inspect expiry and joining status, and explicitly accept, decline or cancel |
| Group management | Owner removal, member Leave, invitation cancellation/dismissal, terminal Close and local Delete |
| Connections | Edit durable LAN/TCP profiles and inspect saved intent separately from attachment state |
| Nodes | Inspect This phone and pair or manage authorized Remote Control targets; distinguish current pairing activity from bounded last-attempt outcomes and saved access |
| Network activity | Read connections, traffic, routes and announcements without treating connectivity as delivery evidence |
| Phone setup | Create or import the phone identity, then follow selected-identity messaging setup, activation and connection steps; explicitly review and confirm a local installation reset |

The product targets native mobile platforms. Supported positioning controls are
part of node management. Independent browser/desktop consumers use the reusable
SDKs and libraries.

## Conversations and authority

A direct conversation is an ordinary two-member MLS group. The creator retains
owner authority and can invite more members through the same enrollment flow.
Simultaneous initiation creates separate groups; the app does not merge their
membership, histories or authority. Saving a contact does not join a group:
invitation acceptance is a separate decision.

Each group has one immutable owner and owner-ordered membership changes. Members
have independent endpoint credentials. RemoveMember and Close prioritize
revocation over a delivery drain. Leave, Close and DeleteLocal have different
consequences: deleting a local copy does not revoke membership or securely erase
other participants' data. Saved departure and terminal-notice obligations can
outlive visible local state. Readmission uses fresh credentials. See the
[group lifecycle and enrollment guide](../../prns-groups/enrollment.md).

Local history remains until explicit deletion, subject to the selected archive's
record and byte budgets. Exhaustion reports capacity failure rather than evicting
earlier messages. Message details retain the original membership reference and
show device confirmations independently of later removal. Available recorded
events are message-specific, ordered locally, and may be incomplete; their times
are not recorded. Displayed message time comes from the sender’s clock. Current
connection phases and routes are not historical evidence for one message.
Delivery expiry is a separate protocol condition. New members
do not receive earlier history automatically. Offline delivery requires a later
communication opportunity with an enrolled peer retaining the ciphertext; the
app promises neither delivery beyond its protocol window nor a blind relay.

The app selects group-service contact claims and group text records.
LXMF is an optional standalone protocol and is not selected as a
second conversation engine. The generic
[contact-card codec](../../prns-contact-card/README.md) supports arbitrary services;
the app owns address-book policy and presentation.

## State and recovery

Rust owners retain protocol state, accepted actions, history and cleanup work.
The UI owns aliases, trust choices, selection, viewed anchors and drafts. Identity
switching does not move conversations, merge contacts or change Remote Control
grants. The product address book stores at most 256 contacts per identity; this
is independent of group membership capacity. Request IDs and encoded intents
remain bound to their captured identity; an uncertain result does not authorize submission with a new ID.

The app saves one pending input before submission and reconciles it with native
retained results. A failed save or unreadable input blocks fresh actions. If the
native owner reports an expired retry with no retained result, the UI explains
that the action may have completed and offers an explicit acknowledgement before
clearing the matching saved input. It does not replay the action automatically.

Drafts are stored per identity and conversation. Recovery of an accepted action
does not clear a newly written draft with identical text. Native history and
operation recovery do not depend on presentation storage succeeding.

Installation Create and Open are distinct. Open validates persisted state and
does not create missing keys or repair unknown formats. No importer or migration
from other application formats is supplied. A raw 64-byte Reticulum private
identity file can provision the controller identity of a new installation. It
does not import named messaging identities, contacts, groups or history.
Newly written state and accepted work
must recover across restart without a reset. Platform ownership and shutdown are
described in the [platform guide](../prns/platform/README.md).

Local data opens before network admission. Stopping networking retains the same
identity and conversation clients, so saved contacts and history remain readable.
Bluetooth can be selected or changed independently of saved LAN/TCP connections.
Manual and periodic identity announcements share one persisted scheduling budget
per host; the default interval for a newly configured identity is 60 minutes.

Reset permanently deletes this phone's local installation after explicit review
and confirmation. It is not a group lifecycle action and does not revoke other
devices or erase peers' copies. An interrupted reset blocks fresh setup. After a
reload, native deletion requires a fresh review; only preference and presentation
cleanup may resume automatically when the installation directory is absent.

## Resource policy

[Product policy](../prns/chat-app/src/policy.rs) selects local resource budgets;
those values are not universal domain limits or promises of phone capacity.
The current catalog sets `max_groups: None`, while catalog bytes, transaction
work, invitations, retained operations and archive storage remain bounded.
The product budgets for four catalog identities with explicit shared resource
reservations. Active network work is bounded separately from stored
conversations.

The Create form starts at 256 members and accepts an explicit per-group member
capacity. `applicationGroupProfile` resolves canonical encoded bounds and rejects
profiles exceeding the selected carrier budget. The number is a starting choice,
not a global 256-member ceiling. Embedded consumers select their own profiles and
measure their own memory, storage and transport constraints.

Summary and member APIs return bounded pages. The UI keeps a bounded cursor
window, offers First/Previous/Next with error-only retries, and loads selected
detail separately from the summary page. These pages are a live traversal, not a retained global
snapshot. Group profile and control data remain native-owned.

## Current limitations

The application is an alpha with a foreground acceptance scope. Protected key
storage, supported backup/restore, ownership transfer, attachments, message
notifications and linked-device synchronization are not provided as complete
product workflows. Native restoration and foreground-service mechanisms do not
guarantee background delivery. Device, radio and signed-package acceptance must
refer to the actual build, as described in [validation](validation.md).

Multiple named identities on one phone are not synchronized copies of one person
across devices. Importing a private key is provisioning, not authenticated device
enrollment or state transfer. There is no automatic contact/group/history
transfer to a replacement phone. Removing a participant cannot erase plaintext
or keys it already received.

The configured production iOS build requests multicast alongside Bonjour;
its signed entitlement and provisioning profile must agree. Development builds
use Bonjour without that entitlement. Neither configuration nor a connected
radio proves message delivery or compatibility with every external peer.

### Contact and support workflows

Contacts opens on the saved address book, with Find people and My card available
from its header. Find people lists recent compatible announcements; it never
automatically saves people. Existing saved contacts keep their private names and
open directly. New contacts require card review and an explicit Save.

Add contact offers scanning or pasting a
signed group-service card, followed by review and an explicit Save. My contact
card presents the current identity’s signed link and QR code. Contact details
keep private labels and Pin/Unpin preferences on this phone, scoped to the full
messaging identity; updating a card or its label preserves its pin. These
preferences are not included in shared contact cards.

Phone setup distinguishes open local data, configured and active messaging, and
running networking. Its next-step links do not activate an identity or change
connections automatically, and a running network does not prove peer reachability.
About and diagnostics → Share diagnostics includes build details, Bluetooth selection/permission/
radio/attachment state, notification permission and aggregate peer, route and
link counts. Unavailable observations are marked unknown. The report excludes
identities, addresses, credentials, messages, discovery-group names and file paths.

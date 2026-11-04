# Authenticated group enrollment

## Scope and use

A consumer can create a group locally, invite a specific Reticulum identity,
and accept or decline invitations through the ordinary group client. Each identity generates its own MLS
credential locally; joining devices generate their own KeyPackage. No private enrollment fixture is transferred.
Group creation and the user's decisions are durable while offline. Invitation
delivery, joining and control catch-up require a running node and eventual contact
with the owner. Established members can exchange current-era content with each
other while the owner is offline. Saved contact names are display hints, not proof
of a person's identity.

The policy has one immutable owner and owner-ordered single-member changes.
Catalog storage and queries are indexed and bounded independently of the selected
member ceiling. See the [construction and resource guide](usage.md) for local
limits, carrier support and working-set selection.

The default catalog has no group count quota and selects 2 MiB of logical catalog
bytes. `CatalogLimits` selects an optional group quota, byte/row/transaction/work
budgets, the unsolicited-invitation inbox and canonical namespace storage limits.
Invitation and result capacities are not derived from the group count. Ordinary
reopen preserves that selection. Each `Intent::Create { title, profile }` selects
an immutable `GroupProfile`; the library default allows eight current members,
and callers may select another checked profile. `resolve_group_profile` derives
the encoded record and enrollment bounds from a requested member ceiling and
explicit byte policies; actual admission still checks local resources.
Invitations have seven-day
validity. Issued invitations reserve available membership slots. New
members receive current-era content after joining; earlier history is not
transferred. Later additions can also interrupt a delayed join's catch-up across
intermediate eras. Ownership transfer, transferable invitation links, history sharing
and concurrent administrators are not supported. Admission funds cleanup
and terminal-delivery obligations from the same catalog byte budget; ordinary
traffic cannot spend those reserves.
The managed enrollment protocol uses v3, catalogs use v5, and the optional
retained-operation schema uses v3. Earlier formats are rejected
without deleting data. Opening does not reset or migrate unsupported groups. Every participating client
must use a compatible version; preserve unsupported data for explicit recovery
or replacement decisions.

`Client::execute` consumes the canonical `Request` and separate
`ExecutionContext`. `Intent::Create`, `Invite` and `Respond` cover enrollment;
`Client::invitations` reads bounded inbox pages; `Client::invitation(id, now)`
reads one invitation independently of the loaded page. Both project expiry using
the selected time and persisted time floor, without maintenance or writes.
Dismissed and locally deleted invitations return no visible record. A scoped
client also enforces its selected group and current control. Management uses the same request
contract. `create_request_id` supplies a 32-byte token: `PGR1`, its creation time
in seconds, and 20 random bytes. Preserve the complete immutable request after
an ambiguous result; tokens are not authentication credentials. When retained
submission is explicitly selected, `Client::submit` captures and persists the
request and `Client::operation` observes its state. A saved acceptance is local
durable intent, not enrollment at the remote device. A group becomes usable
after committing MLS state through its required membership checkpoint. The
generated client exposes these same owning declarations; private MLS keys,
KeyPackages and Welcome bytes are not part of that foreign API.

`Client::conversations` returns bounded summary pages with explicit continuations.
`Client::conversation` selects one group's detail independently of its list page;
`Client::members` returns a separately bounded member page and its `control`.
Continue through an empty page when it has a continuation. A changed control
between member pages requires restarting that roster traversal.
Each member carries its exact endpoint and incarnation,
Reticulum identity, and local/owner flags. The roster is taken from the same
validated configuration as the group title and authority; duplicate identities,
missing endpoint mappings and exceeded membership limits fail rather than produce
a partial roster. Fixed fixtures have no owner. The canonical member record
preserves these fields; a consumer can resolve display names separately. Neither a
contact name nor inclusion in this roster means a device is connected. The
existing conversation `revision` is a record/change revision, not a membership
revision; use `control` for membership provenance. Cached membership can
still precede updates held by other devices.

`Error::rejection_reason` distinguishes definite invalid input, no recipients,
request-input conflict, expired invitation, missing item, membership conflict,
non-owner, incomplete joining and failed authentication. A request-input conflict
means the same request identifier was presented with different input; it is
separate from inviting an already enrolled or already invited identity. Capacity,
busy, uncertain storage/transport outcomes and expired retry evidence retain
their existing distinct recovery paths. Never infer that an uncertain action did
not commit or replace its request token automatically.

The ordinary client returns `ApiError::Rejected { reason }` for a definite
rejection. The optional retained facility stores the same canonical
`RejectionReason` in `retained::State::Rejected`; uncertainty remains
`State::Uncertain(ApiError)` rather than a failed-result shortcut. Exact completed
replay returns the recorded result. A caller must explicitly decide whether to
admit a new action after rejection or expired retry evidence; it must not
replace the old request ID automatically.

## Authentication and ordering

Enrollment uses `prns.groups.enrollment`, a separate Reticulum `SINGLE` destination,
with identified encrypted Links and bounded request/Resource frames. Opaque MLS
bytes use base64 inside JSON. Their selected MLS byte bound and the complete
enrollment-envelope bound are part of `GroupProfile`; its consistency check sizes
the actual encoding including both signed rosters. The local catalog selects a
separate ingress byte budget, checked before decoding. A peer that cannot support
an offered profile returns `UnsupportedProfile` before creating local membership
or a KeyPackage. Join pins the offered profile digest, and signed successors
cannot change it. The carrier
checks the announced destination identity against the expected primary identity;
ingress rejects unidentified requests before copying or queueing their payloads.
A claimed identity inside JSON is never transport authentication. Discovery and
routing retain the existing Reticulum protocol; this adds no core packet format.

The initial directed offer binds the owner's MLS signing key to its authenticated
Reticulum Link identity. Explicit acceptance pins that owner and key. This is
identity-key authentication, not a claim that a saved contact has been verified in
person. An invitation is not a transferable bearer token or an independently
verifiable, offline invitation file.

The owner authenticates acceptance from the invited primary identity and binds it
to the recipient's exact credential and KeyPackage. An owner-signed membership
manifest binds the group, title, parent head, revision, MLS epoch, Commit digest,
and complete mapping of Reticulum identities to MLS credentials. An addition must
preserve every prior member, add exactly one member, and advance the epoch once.
A removal must remove exactly the named endpoint/incarnation, preserve all other
bindings including the owner, and advance the epoch once.
The adapter checks the **actual resulting MLS roster**, not just an allowed
superset, before committing application facts with the crypto state. Existing
members apply a contiguous chain of controls before using its new descriptor;
wrong parents, transitions conflicting with the locally accepted history, and
substituted credentials fail closed. This does not provide consensus or prevent
a compromised owner from presenting separate histories to disconnected members.

The owner retains the exact Welcome and membership result for a consumed invite.
A retry cannot enroll a second member or substitute another KeyPackage. A delayed
Welcome includes the owner's current signed tip; the joining device stays out of
the conversation list until it applies the chain through that tip. The owner shows
“Device joined” only after an authenticated acknowledgement from that device's
durable joined state. Control catch-up transfers one bounded transition at a time. The network runner refreshes
membership from durable state and fences each asynchronous content operation to
its starting control head. A membership change committed locally during that
operation forces a retry. Members can continue using cached membership until
updates reach them; this does not require globally current membership or an online
owner before every content send.

Completion reports scheduling activity only after correlating the pending work
and validating/committing its result. An unchanged control response is idle and
does not wake content synchronization; a committed join, invitation update or
new control is progress. An unchanged control response itself writes no group
state; independently due retention/recovery maintenance can still run.

## Recovery and delivery

One `GroupManager` owns the existing native database lane and one
`RedbStoreOwner` across all group namespaces. Catalog intents and local private
credentials are persisted before crypto output can escape. MLS Create, KeyPackage,
Add, Join and received control transitions stage their enrollment facts in the
same atomic operation as the MLS mutation. Network waits hold no database lease.
Dropping an admitted foreign caller or stopping a network waiter does not cancel
its admitted durable operation.

API results remain replayable while retained. Maintenance retires results after
14 days and commits the deletion with a monotonic time fence, so an old token
cannot become a new operation after a clock rollback. An expired retry fails
explicitly; it must not silently generate another token. Existing opaque request
IDs receive a persisted 14-day migration grace; exact retained retries work, but
unknown opaque IDs cannot admit new work. Durable Create and acceptance intents
recover independently of their API result records.

Pending direct publication requests must retain their original
`Request::original_scope`, including its `ControlReference` and exact local
`StoreIncarnation`, when retried across
restart or rejoining. `Client::at_control` scopes reads and carrier work; it does
not replace mutation authority captured in the request. `GroupManager` uses the
incarnation index to select that one retained namespace, then recovers a committed
exact result before checking the current membership and control fence. It does not re-publish that record. Unknown work must pass the
original control fence; token creation time alone cannot identify its membership.
When an embedding selects retained submission, the group-owned coordinator
resolves invitation actions to their group and preserves their exclusion until
the exact outcome is terminal. The embedding supplies the canonical
authorization/admission adapter on the same storage lane; merely opening a
standalone `LocalOwner` does not initialize that optional schema. There is no
separate app action model or journal in this contract.

Locally issued and explicitly accepted invitations consume their selected byte
budgets and fund their own cleanup obligations. By default unaccepted incoming
offers share four slots, at most two per
authenticated peer. Declining retains its suppression record until the offer's
seven-day expiry. Maintenance then prunes unaccepted/declined offers and persists
an invitation clock fence against redisplay after rollback. Confirmed joins and
definitively expired accepts retain catalog records for 14 days after that state
is recorded. Unfinished accepted joins and owner-committed Adds remain protected:
an invitation deadline cannot undo an Add or discard its exact Welcome.

Enrollment output cleanup checks the durable snapshot before retiring redundant
Create, KeyPackage, Add, Join and received-control outputs, in batches of at most
sixteen per group. Exact signed controls and Welcome bytes remain in indexed
namespace records;
service recovery facts and ciphertext outputs are excluded. Catalog retirement
and each group's output retirement use separate atomic transactions and safely
resume after interruption. This is logical storage reuse, not physical erasure.
The native owner finalizes backend receipts only below the oldest retained MLS
output. New MLS work leaves two receipt slots for cleanup; the last slot can only
retire that oldest output. An already-full older database with no safely finalizable
prefix fails closed rather than forgetting a live obligation to manufacture space.

Add remains an authorization barrier: old receive epochs and old application or
proposal output retries are fenced. Retained Commit/Welcome replay remains
available for control and enrollment recovery. Existing plaintext history and authenticated receipt counts remain
local. Old-era records cease to be offered, receipt emission stops, and logical
receipt reservations are recalculated for the new era. Unfinished messages are
shown as **delivery interrupted by membership change**, never as delivered. The
service does not repair unfinished old-audience content across an Add; it
must not re-encrypt it into the enlarged group and disclose it to the new member.
Immediately before an Add, the owner checks its durable current-era authored
records. A record with missing receipts blocks the Add until its receipts arrive
or it definitely expires, including records sent after the invitation was issued.
The bounded service query uses publication facts and receipt evidence, so retiring
retry ciphertext cannot masquerade as delivery. Unknown or overlapping time bounds
do not establish expiry. The check and Add run on the same exclusive storage lane.

A blocked Join returns `AwaitingDelivery`. Both endpoints retain the exact Join
hash and KeyPackage across restart; retries with changed input fail. Invitations
expose `waiting_for_delivery` without changing their existing state. Repeated
deferrals are idle, preserve normal retry backoff and do not write again. A
committed Add always replays its Welcome before this check, even if later records
are pending. Deferral does not extend the invitation deadline; an uncommitted
Join can still expire. The visible waiting flag ends at expiry or committed join.

This protects the owner's known authored deliveries, not a group-wide drained
state: another member can still have an offline send the owner has never seen.
Continuous owner traffic or a missing receipt can postpone joining until the
record deadline, potentially past the invitation deadline. Finish
important sends before inviting and coordinate additions with existing members.
There is no addition override that silently abandons pending delivery or re-encrypts
old-audience history for the new member. Removal and closure do not wait for a
delivery drain: revocation takes precedence, and interrupted deliveries retain
their original evidence.

## Management and local lifecycle

The canonical `Intent` API includes `RemoveMember`, `Leave`,
`CancelInvitation`, `DismissInvitation`, `Close` and `DeleteLocal`. Their successful
local result is `Saved`; this never claims every remote device has caught up.
Conversation projections expose `Active`, `Leaving`, `Removed` and `Closed`,
plus authoritative action capabilities. Only an active group with another current
member permits sending. Fixed provisioning has no owner management authority.

- **Remove member:** only the owner can remove another exact endpoint/incarnation.
  The request captures the displayed `OriginalScope::Control`; its expected head
  and authorization era, together with the local store incarnation, reject stale membership.
  It commits the MLS Remove, signed roster, exact control and terminal notice
  together. Remaining members catch up through the ordered control chain. Removed
  members obtain a separate owner-signed terminal proof without newer MLS secrets.
- **Leave:** a non-owner saves a durable departure request and immediately stops
  local publication and content synchronization. The owner authenticates the exact
  member binding and commits the same Remove operation. An offline owner leaves
  the request pending. Delayed requests cannot remove a fresh rejoined incarnation.
- **Cancel invitation:** the owner can cancel only before Add commits. The check
  and change run on the same storage lane as Join. A consumed invitation reports
  `AlreadyJoined`; its member must be removed explicitly. Cancellation suppresses
  delayed offers and requests, including when cancellation arrives before an offer.
- **Dismiss invitation:** hides a finished invitation locally without discarding
  enrollment or cancellation replay evidence.
- **Close:** the owner commits a terminal signed lifecycle decision and retires
  local group crypto state. It stops further enrollment and content use as devices
  learn the decision. Pending invitations become cancelled; a group ID never reopens.
- **Delete local:** removes local conversation content and private namespaces.
  Owners close first. For an active non-owner, deletion itself does not remove the
  remote membership; use Leave first when departure is intended. A previously saved
  Leave continues after deletion. Tombstones and pending terminal notifications
  survive outside the deleted namespace and do not consume active group slots.
  Pending unjoined entries use their exact `OriginalScope::Unjoined` incarnation;
  a delayed cleanup cannot delete a replacement local membership.

With the optional archive selected, removed and closed conversations retain
readable local history until explicitly deleted. Without it, reads remain
bounded by protocol retention. Fresh readmission after removal uses a new
credential, endpoint and store incarnation. The previous namespace remains a read-only history source for the
conversation; it is never reset into a fresh crypto session. Text history pagination
keeps distinct local sequence ranges for each incarnation. Deleting the conversation
retires every local incarnation of that group.

Deletion first commits a durable fence, then purges only the exact owned namespaces
and cursor rows. Cleanup resumes after interruption. Stale enrollment/network
completions cannot recreate them. Native deletion is logical reuse, not physical
erasure of database pages or plaintext already held by other devices.

There is no seven-transition or other lifetime membership-change count. Control
and Welcome history use indexed records within the selected namespace budgets;
individual rows and cryptographic snapshots retain separate byte bounds.
Admission reserves snapshot bytes, native objects/bytes/receipts and MLS
output slots for removals and closure. Ordinary writes cannot spend that capacity.
Catalog results and tombstones share the selected catalog byte budget, 2 MiB
by default. These limits can still report capacity; they are not filesystem-space
guarantees.

Updates remain eventual. Devices using an older cached membership can communicate
until the new control reaches them. Removal protects the subsequent MLS epoch;
closure/removal cannot erase plaintext or keys a device already received.

## Boundaries and remaining qualification

The generic core's invitation-time and addition-transition checks are allocation-free
`no_std`. The MLS adapter and record service use `no_std + alloc` with storage
and admission limits; those limits do not bound all transient MLS allocations.
The enrollment catalog, JSON carrier, native database and mobile scheduling are
`std` adapters; they do not become requirements for embedded implementations.
An embedded enrollment consumer can reuse the core checks and MLS transaction
seams with its own bounded representation and durable backend. The complete
enrollment catalog, wire codec and scheduler have not been implemented for embedded
targets.

The native backend still stores private state without application-level encryption
or physical key erasure. Removing fixture transfer does not establish protected
phone storage, secure backups or at-rest forward secrecy. Group membership and
Welcome contents are carried inside encrypted Links; relays still see ordinary
Reticulum routing and traffic metadata. There is no anonymous enrollment or blind
relay custody in this version.

Storage limits are independent ceilings, not guaranteed usable capacity. Strangers
can still exhaust the four-slot unsolicited inbox, including through multiple
identities; these quotas do not provide spam filtering or identity admission.
Local deletion releases the visible group slot and its private
namespaces while retaining the small suppression and notification records needed
for recovery. The bounded control/Welcome history remains for each live group's lifetime. A full
request-result window or protected invitation quota reports capacity instead of
evicting a live recovery obligation. Host tests cover aborted catalog transactions,
database reopen, retirement/clock rollback, repeated invitation cycles and exact
Welcome recovery across retirement; they are not physical power-loss evidence.
Membership tests also reopen both sides after a lost deferral, preserve a Join
issued before an intervening send, keep Add blocked after delivery without its
receipt, and replay a lost committed Welcome despite later pending records.

Host recovery and standalone foreign-consumer tests do not qualify mobile
background execution, Python-router application interoperability or production
security. Physical acceptance requires separate evidence tied to the exact
consumer, build, devices and network. The version-3 wire encoding has not been qualified for
external application interoperability.

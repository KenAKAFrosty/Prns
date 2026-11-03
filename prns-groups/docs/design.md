# Group architecture

The group libraries distribute authenticated application records over Reticulum.
The [owning library](../usage.md) adds enrollment, durable requests, history and
scheduling. Lower layers can be used independently with caller-supplied membership
and storage.

## Ownership boundaries

| Layer | Current responsibility |
| --- | --- |
| `prns-groups-core` | Allocation-free IDs, control references, request tokens, clock admission, delivery progress and path/work policy. |
| `prns-groups-store` | Recoverable transactions, bounded staging, operation receipts and backend contracts. |
| `prns-groups-store-redb` | Caller-owned redb namespaces, atomic participant writes, change stamps, quotas and optional validation caching. |
| `prns-groups-mls` | MLS credentials, encryption, authenticated roster checks and exact committed outputs over the transaction coordinator. |
| `prns-groups-service` | Application records, immutable delivery targets, authenticated receipts, retention and inventory over an enrolled MLS group. |
| `prns-group-text` | Borrowed UTF-8 encoding and text projections over generic records. |
| `prns-groups-reticulum` | Authenticated Link exchanges and bounded inventory reconciliation using a borrowed host. |
| `prns-groups` | Owner-managed enrollment, indexed local state, retained requests, history projections and optional network scheduling. |

Core and text are allocation-free `no_std`. The coordinator, MLS adapter and
record service use `no_std + alloc`. The redb adapter, managed owner and concrete
Reticulum carrier use `std`. Portable types do not provide an embedded network
member by themselves; [embedded support](../core/docs/embedded.md) describes that boundary.

## Identity and authority

A group has a stable ID and one immutable owner. An enrolled member binds a
Reticulum identity to a distinct MLS endpoint, incarnation and signing key.
An authenticated control reference identifies the membership state under which
an operation was admitted. Local store incarnation is a separate recovery fence.
Rejoining after removal creates new credentials and a new namespace; old local
history does not become fresh membership.

The managed profile allows owner-ordered, single-member additions and removals.
Invitees generate their own KeyPackages. A signed roster and its matching MLS
transition commit together. The owner signs terminal removal and closure proofs;
a copied name, contact card or routing identity does not establish membership.
See [enrollment](../enrollment.md) for the complete lifecycle and replay rules.

## Durable publication and recovery

One coordinator transaction contains MLS state, application admission, exact
ciphertext, output obligations and the operation receipt. The redb participant
also stages the managed catalog and archive facts in that same physical
transaction. No provisional ciphertext or application effect may be published
before a definitive commit.

A failed attempt discards its working crypto state. An uncertain commit fences
new work until recovery settles it. Retry returns the exact committed result;
it does not encrypt again or choose a replacement request ID. Public retained
requests preserve the complete original scope, including local incarnation.
See the [transaction contract](../store/README.md) and
[record service](../service/README.md).

## Records and delivery

Applications supply the record namespace, schema, body and effects. A record
retains its original author, control, target incarnations and validity window.
Targets describe receipt obligations; they do not restrict the MLS encryption
audience. Every current MLS reader can decrypt the application record.

A durable receipt proves acceptance by its exact endpoint incarnation. Link or
Resource success proves transport work, not durable application acceptance.
Neither proves human reading, successful application effects or delivery to all
members. Removal and expiry do not count as receipts. Applications using commands
must implement their own idempotent effects after durable record admission.

## Network execution

The carrier uses ordinary Reticulum SINGLE destinations, identified Links and
requests/Resources. Existing Reticulum routers can carry those exchanges, but
an LXMF or other application is not thereby a group-protocol peer. All groups
under one local identity share its group and enrollment service destinations.
Announcements carry reachability information, not rosters or content.

The caller owns the host, entropy, clock and scheduling lifetime. The standalone
owner supplies a serialized storage worker; it does not create a Reticulum node.
The network runner borrows host authority and performs bounded group/peer work.
It explicitly schedules recovery and cleanup; reading an enrollment-work page
has no maintenance side effect. See [transport](../transport/reticulum/README.md)
and [owner construction](../usage.md).

## Resource selection

`GroupProfile` authenticates per-group member and encoded-byte limits.
`CatalogLimits`, `RedbLimits`, client input limits, query page limits and cache
budgets govern local resources. They are separate policies. A declared member
ceiling does not preallocate that many memberships or guarantee that every roster
fits the selected byte budgets. Admission funds actual membership and cleanup
obligations before publishing changes.

Catalog and invitation queries are paged; exact detail and member pages do not
require loading every conversation. Generic record/message pages currently have
a row bound, not the same returned-byte contract as catalog pages. Logical
quotas exclude allocator overhead, temporary copies and physical database pages.
The [policy guide](policies.md) explains those distinctions.

## Scope

The implemented private profile does not provide owner transfer, threshold
administration, public anonymous writing, automatic key-refresh scheduling,
permissioned history transfer, a blind-custodian service or a general replication
engine. Native redb storage is plaintext. Logical deletion, crash recovery and
receipt retirement do not establish secure erasure or rollback protection.
The [security guide](../security/docs/security.md) describes the resulting
trust and confidentiality limits.

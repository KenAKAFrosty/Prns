# Group service foundation

`prns-groups-service` joins immutable application records, delivery evidence and
MLS state in one recoverable transaction. It accepts opaque application bytes:
chat and telemetry use the same service with different namespace/schema IDs.
The crate is `no_std + alloc`; `prns-groups-core` remains allocation-free. Storage,
identity, entropy, scheduling and transport belong to the caller.

The service consumes an already enrolled MLS adapter and a caller-authenticated
descriptor. `new` requires fixed membership. `new_managed` keeps the group, local
identity and limits fixed while accepting a descriptor advanced by an external
authenticated control transaction. The caller must
ensure that the descriptor matches the actual MLS enrollment; the service does
not bootstrap or verify that correspondence. It rejects incoming MLS control
changes. The private `PGS` encoding is a testable provisional format, not the
published interoperable group protocol.

## Using it

1. Open one exclusively owned endpoint coordinator and MLS adapter. On a native
   host, [`RedbStoreOwner`](../store/redb/README.md) borrows the application's
   existing database; the library creates no separate runtime or database.
2. Construct `GroupService` with the authenticated fixed reader set and explicit
   limits. Reader and delivery target sets must be sorted and unique. Configuration
   is persisted on the first operation and must match on reopen. For managed
   enrollment, commit membership and MLS together first, then open `new_managed`
   with that committed descriptor; unverified network manifests are never valid
   constructor input.
3. Allocate an operation ticket from the coordinator's current head. `publish`
   commits the record, pending delivery evidence, change event, crypto changes,
   retained-envelope inventory and exact ciphertext together. Save the operation
   ID before dispatch.
4. Fetch `outgoing(operation)` only after success. After a lost result, recover
   the operation with `coordinator().recover(operation)` before any other work,
   then retry the same request/operation. This returns the original ciphertext;
   changed request data produces `IdempotencyConflict`.
5. Feed incoming ciphertext to `receive`. Acceptance commits the body and a
   pending receipt intent, exact incoming envelope and consumed receive keys.
   Call `emit_receipt` in a separate transaction, then send its exact durable output
   to interested peers.
6. Resume `changes(cursor, page_size)` after restart or a missed notification.
   `Published` and `ReceiptQueued` include their operation IDs for recovering
   dispatch work; `Received` identifies records with pending receipt intents.
   `progress(record, clock)` reports authenticated device receipts separately
   from pending, expired and time-uncertain obligations.

`dependencies()` reads durable obligations without a clock, maintenance or writes.
It separates current-era authored records/targets awaiting authenticated receipts
and local receipt intents from passive retained record/indexed-result counts.
Wall-clock observation alone never clears this conservative report; completed
receipt evidence, actual maintenance removal, an authorization-era barrier or a
terminal MLS fence does. A retired namespace reports zero active network work
while preserving its passive counts. The bounded scan validates its record index,
flags and local receipt evidence; errors mean incomplete evidence. It does not
count every backend receipt or MLS output, and does not prove a native carrier,
identity service or owner has drained. An embedding combines it with those owners'
independent facts under its own revision/exclusion boundary.

An exact duplicate envelope returns its previous outcome without consuming a
new ticket. Always derive the next ticket from the coordinator's current head;
do not advance a private counter merely because `receive` returned success.
Callers serialize service access and must not mutate its reserved application
records through another adapter.

`snapshot()` returns `None` until the first durable application operation, then a
persisted inventory generation and frozen head. Page with
`inventory(snapshot, after, kind, recipient, scan_limit)`; it scans at most the
specified number of entries, so a filtered page may be empty before reaching its
head. Each returned entry carries its own position. Advance to `page.next` only
after every returned item is handled, or checkpoint the last handled entry when
stopping partway through a page. New arrivals do not alter the frozen snapshot.
Restart/reinitialization of the underlying store invalidates a previous
generation with `RestartInventory`. Expiry maintenance also changes the generation,
so a peer restarts its snapshot after retirement. Positions remain monotonic and
can have gaps; page sizes bound live entries examined, not the numerical cursor
distance. Change queries likewise skip deleted positions while preserving every
retained record's publication/acceptance entry.

Scan records before receipts using the same snapshot. `envelope(id)` supplies the
original MLS ciphertext, including envelopes accepted from a different author;
no re-encryption is required for a surviving peer to repair an offline endpoint.
`envelope_id(bytes)` verifies an offered ID. `envelope_info(id)` supplies
authoritative local kind/record/size facts for checking a known envelope against
remote inventory hints before skipping a redundant fetch. `has_envelope(id)` is
also available for simple presence queries. Echoes of one's own records/receipts are recognized before MLS processing.
Inventory items and transfer success never establish remote durable receipt.

The caller authenticates the transport peer and binds it to an enrolled endpoint
before exposing inventory or fetched bytes. Inventory filters records and receipts
to the original author or selected delivery targets for scheduling. This is not a
confidentiality boundary: every enrolled MLS reader can decrypt every publication.
No blind-relay admission or unauthenticated inventory is provided.

A receipt carries its original record deadline inside the authenticated MLS body.
It must match any retained record's deadline. Before that deadline can be proven
expired, a missing record returns `MissingRecord`; uncertain record-admission
time returns `TimeUncertain`. Both abort the transaction and leave the ciphertext
retryable. The transport must retain/reoffer that input after the record or trusted
clock becomes available.

Authenticated invalid or expired content is durably rejected. An authenticated,
otherwise valid expired record also leaves a group-scoped tombstone for its
canonical record ID. A receipt depending on that terminal record is durably
rejected as `UnknownRecord`, so a records-first synchronization cursor can continue
after reopen. An inventory claim, missing record, untrusted clock or unauthenticated
bytes cannot create that tombstone. An unknown receipt whose authenticated deadline
has definitely expired is rejected even after its record and tombstone have been
reclaimed. Neither rejection creates receipt/progress evidence.

An expiry-rejection fact without an authenticated record ID cannot be safely
backfilled from an inventory hint. Authentication, capacity and storage failures
produce no durable acceptance. An uncertain commit requires recovery before
further processing.

**Wire versions:** records use version 1 and receipts use version 2. Unsupported
receipt versions are rejected. Expiry tombstones use the deadline-bearing version
2 storage format; unsupported stored formats fail without replacement or migration.

## Bounded expiry maintenance

Call `maintenance_plan(trusted_clock, max_records, max_operations)`, skip empty
plans, then `maintain(new_operation, &plan)` on the exclusively owned service.
The planner scans only live indexes and the bounded MLS output list. It selects
definitely expired records oldest first, prioritizing the record behind the
earliest retained service output so a coordinator recovery fence can advance.
Only outputs belonging to this group are eligible, even when groups share an
adapter and coordinator.
Pending records remain intact until their deadline. The plan binds the current state, and a stale plan fails before
deletion. A typical native pass selects one record and four operations.

One atomic transaction removes selected bodies, receipt intents/evidence and
publication entries, retires selected exact MLS outputs and operation facts, and
updates quotas, live indexes and a monotonic expiry floor. A retained record's
deadline is not extended by retry. Deletion is not delivery: no missing receipt
becomes a received receipt. `expiry_floor()` lets a caller apply the same persisted
clock floor to its own request-retention policy; a clock rollback cannot resurrect
expired content. This policy bounds the service's delivery and inventory cache.
The managed group's optional [archive](../usage.md#features-and-construction)
retains local application history until explicit deletion, independently of
delivery expiry. Neither policy establishes physical secret erasure.

Many repeated ciphertexts may refer to one expired record. Its body and forwarding
eligibility end in the first batch; later bounded passes remove remaining
envelopes, receipt events and operation facts. An operation remains in
`pending_operations()` until its output and service fact retire together. This
service does not retire enrollment/control outputs or coordinator receipts; the
caller retains their recovery dependencies and advances its receipt fence only
when safe. Service rejection facts admitted with an unknown clock get one full
`maximum_window` of retention from their first trusted maintenance observation.

Versioned live-position indexes migrate atomically on the first mutation without
changing persisted `Limits`. Monotonic heads continue advancing beyond capacity;
only live entries consume their quota. Maintenance advances the change head even
when it produces no event, so history projections notice deletions. Lost maintenance
results can replay the last exact plan/operation; older pruned evidence fails
closed. The caller must recover an uncertain commit before proceeding. Empty
polls do not write clock floors or consume operation receipts.

MLS output reservations include all service groups in the adapter's bounded
group catalog, read within the same transaction. When an existing group lacks
its reservation counter, other groups fail admission closed. A bounded maintenance pass reconstructs that group's
counter without creating MLS output; run it for each affected group before
resuming normal traffic. Reconstruction does not repeat while the counter is present.

## Guarantees and limits

- A receipt means the selected endpoint incarnation durably accepted the record
  and inline body. Local publication, transport success and receipt encryption
  do not prove remote receipt, application execution or human read state.
- All enrolled MLS readers can decrypt a publication. Selecting fewer delivery
  targets does not restrict its encryption audience. Receipt sender, record,
  group and original target incarnation are checked before evidence changes.
- Logical record IDs bind canonical metadata, targets, nonce and body. Operation
  IDs bind immutable requests; exact-envelope deduplication prevents ratchet
  replay and duplicate change events. No automatic re-encryption/repair exists.
- Limits bound body/target decoding, retained record count, feed length, retained
  envelope count and total exact ciphertext bytes. MLS/provider/backend limits also
  apply. An envelope is retained in the same transaction as admission; capacity
  exhaustion aborts without consuming receive keys or advancing inventory. Each
  accepted/published record reserves one envelope, one change entry and
  `MAX_RECEIPT_WIRE_BYTES` (1024) ciphertext bytes per original target, including
  the local target when present. Only that target's first authenticated receipt
  consumes its reservation; new records and rejection events cannot consume it.
  The fixed unpadded MLS profile emits smaller receipt envelopes and enforces this
  receipt wire bound. Admission also reserves MLS output slots for those pending
  receipts plus two spare outputs, checked after staging each operation. Duplicate
  and rejected traffic cannot consume those reserved slots. These are logical
  service/MLS quotas, not physical-space or backend-object reservations. Callers
  still size those independent limits.
  If the reservation counter is absent, the next committed operation reconstructs
  it atomically from retained records and receipt flags. Reconstruction can fail
  if those obligations exceed the selected quota; failure changes nothing.
  Expiry maintenance reclaims history/feed entries; exhaustion before expiry
  rejects further work instead of dropping live obligations.
  Operation facts and envelope deduplication consume backend object/byte capacity.
- Do not retire coordinator receipts or MLS outputs while the application still
  needs their recovery/dispatch evidence. The embedding owns that retirement
  policy; prematurely pruned evidence yields an error, never permission to resend
  under a consumed operation ID. Fetching locally generated retained ciphertext
  rechecks its original MLS output and authorization-era fence; a copied envelope
  cannot bypass output retirement.
- Trusted time and monotonic clock persistence remain caller responsibilities.
  The service does not establish a trustworthy clock across reboot. Retained
  plaintext application history has its own deletion/privacy policy, independent
  of MLS ratchet key retirement. The redb backend stores plaintext and does not
  provide protected mobile secret storage.

## Verification

[Native flow](tests/native_flow.rs) exercises real MLS records and receipts across
independent endpoints, exact retry, tampering, inventory and quota boundaries.
[Maintenance recovery](tests/maintenance_recovery.rs) covers interrupted commits,
reopen, stale plans, receipt reservations and bounded reclamation. Codec tests
cover canonical framing and malformed input. The MLS and redb adapters separately
own crypto rollback and backend crash-boundary tests.

These are host library checks. The [Reticulum carrier](../transport/reticulum/README.md)
owns network exchanges, and the [group owner](../usage.md) adds managed enrollment,
scheduling and foreign consumers. Physical storage protection, mobile execution
and firmware resources require evidence for their actual embeddings.

```sh
cargo test --locked --manifest-path prns-groups/Cargo.toml -p prns-groups-service
cargo test --locked --manifest-path prns-groups/Cargo.toml -p prns-groups-store-redb
```

## Authorization barriers in managed groups

An era change preserves local plaintext and real receipt evidence, but inventory
skips old-era records/receipts and `envelope` does not release them. Old receipt
intents stop emitting; the next operation reconstructs logical reservations for
the new era. `in_current_era` lets application projections label unfinished old
messages as interrupted without manufacturing delivery evidence. This does not
repair delivery across a membership change or remove historical bytes from physical storage.
See [managed enrollment](../enrollment.md) for the owning lifecycle integration.

After local removal or closure, the last active descriptor can reopen the ledger
for history queries. The MLS terminal marker prevents publication, incoming
admission, receipt emission, inventory export and forwarding, including previously
accepted exact retries. `in_current_era` returns false and pending-work queries
stop scheduling traffic, while bodies, changes and authentic receipt counts remain
available. In active managed groups, targets absent from the authenticated current
roster count as removed; removal never creates receipt evidence.

`has_pending_authored_deliveries(clock)` checks bounded live publication facts and
receipt flags before a caller changes membership. It considers this endpoint's
current-era authored records only; missing receipts block until definite expiry,
including when clock uncertainty prevents a definite decision. Received records
and retained old-era history do not block. The query uses durable evidence rather
than disposable network work or cached conversation state. Serialize the check
with the membership transaction; it is local evidence, not coordination with every
sender. The native enrollment adapter defers Add while this query is true.

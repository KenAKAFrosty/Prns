# redb group storage

`prns-groups-store-redb` implements the group coordinator's `AtomicBackend`
contract in the application's existing **redb 4.2.0** database. It is a durable
development and conformance backend. It does not encrypt stored MLS secrets or
provide physical key erasure; it is not yet a production phone secret store.

The app or headless consumer owns the `Database`, its cache configuration and
its serialized work lane. Create one `RedbStoreOwner` from that database, then
explicitly initialize or open a `StoreNamespace`. A backend exclusively borrows
that owner. The application's unrelated tables remain usable through the same
database reference. This crate creates no database, executor, thread or runtime.
Creating duplicate owners for the same database, or writing reserved group
tables outside the backend, violates the exclusive-writer contract.

For persistent attachments whose managers are reconstructed for each job,
`ValidationCache::new(Arc<Database>, NonZeroUsize)` retains a bounded number of
validated namespace headers. Borrow `cache.owner()` for each lexical manager.
The cache is opaque and cannot be cloned. Its retained database allocation is
the physical identity; a copied file, matching incarnation or reused address
cannot substitute another database. Release the attachment and its cache before
reporting physical close, since the retained handle keeps the file lock alive.

A miss scans all values and receipts using bounded row memory. A hit checks
the exact incarnation, persisted limits, revision, retired prefix, logical usage
and table counts. Only a successful owned commit advances cached proof; an
attempt invalidates it before table edits, including participant work and
uncertain outcomes. Deletion and import also invalidate it. Exact recovery still
looks up its receipt. Cache capacity is an explicit embedding choice; deterministic
eviction affects work only and never substitutes permission for validation.
The sorted index uses binary search for hits/invalidation. Eviction rotates
through sorted positions; it is not a recency policy. Only cold insertions and
eviction shift entries; comparisons, shifts and evictions are also measured.

`RedbStoreOwner::new(&Database)` remains strict and uncached. `backend.inspect()`
always scans every row, even for a cached attachment. Neither a cache hit nor
ordinary metadata checks detect arbitrary out-of-contract raw edits that preserve
the header and counts. Strict cold open/inspection retain full corruption checks.
`ValidationCache::stats()` measures full-validation row/byte work and cache hits;
these saturating observations are separate from persisted accounting.

`backend.inspect_with(callback)` visits each borrowed key/value in the same read
snapshot and pass as strict integrity validation. A domain owner can check its
own row prefixes and relationships without collecting the namespace. The adapter’s
row bounds pass before each callback; the result is valid only after the entire
inspection succeeds. Errors and panics invalidate prior cached proof. Callbacks
must not mutate the database or retain borrowed bytes. This is one inspection
pass; a preceding cold `open` performs its own validation as described above.

An attachment that already selected a durable group-domain stamp can use
`owner.with_change_stamp(incarnation)`. Every namespace write, import, retirement
and deletion then checks and advances that exact stamp in its transaction,
without enumerating unrelated database tables. Other domain-owned transactions
use `change::commit_selected` with the same incarnation. Missing, corrupt,
foreign or exhausted stamp state aborts all staged facts. This does not create
the stamp or replace startup validation. Owners without this selection retain
the optional-stamp behavior; ordinary standalone stores need no new schema.

```rust
use prns_groups_store::{Coordinator, StoreIncarnation};
use prns_groups_store_redb::{RedbLimits, RedbStoreOwner, StoreNamespace};

fn open_group_store<'a>(
    owner: &'a mut RedbStoreOwner<'_>,
    namespace: StoreNamespace,
    incarnation: StoreIncarnation,
    limits: RedbLimits,
) -> Result<Coordinator<prns_groups_store_redb::RedbBackend<'a>>, Box<dyn std::error::Error>> {
    let backend = owner.open(namespace, incarnation, limits)?;
    Ok(Coordinator::new(backend, limits.transaction_limits()?)?)
}
```

`RedbLimits` is the owning configuration type. Its seven `u64` fields match
the existing persisted metadata slots; `validate()` checks that they fit the
current host, and `transaction_limits()` provides the checked portable
coordinator limits. Zero permits no usage. Optional `serde` and
`uniffi-bindings` features expose this same declaration for persistence and
generated foreign consumers (`GroupStoreLimits`), without a parallel native
record. Both features are off by default. These limits bound logical payloads,
not allocator overhead, resident memory or physical database size.

Initialization is a separate `owner.initialize(namespace, incarnation, limits)`
operation. The caller supplies a fresh CSPRNG-generated incarnation and retains
the selected namespace/configuration in its account lifecycle. Initialization
refuses any existing namespace table, including orphaned empty tables. Opening
requires the exact persisted incarnation and limits. There is no implicit reset,
replacement incarnation, schema upgrade or format migration.

After the caller durably fences a deleted group's enrollment and network work,
`owner.delete(namespace, incarnation, limits)` deletes that exact namespace's
three tables in one transaction. The owner lease excludes live backends. A fully
missing namespace is an idempotent retry; partial tables, a different incarnation
or different limits fail without deleting anything. Other groups and application
tables remain intact. The caller must retain anti-replay evidence outside this
namespace and must not reuse the old incarnation. This is logical deletion, not
physical erasure or database compaction.
`owner.validate_delete(namespace, incarnation, limits)` performs the same checks
without changing tables, so callers can preflight every owned namespace before
starting a multi-namespace reset. Each deletion revalidates its own transaction.

## Durable contract

Each immediate-durability redb transaction publishes all KV mutations, revision
and receipt together. Crypto state, exact outgoing ciphertext and application
records therefore share one durable boundary. Replaying a committed ticket only
returns its original receipt, even if supplied different mutations. Tickets are
scoped to the store incarnation and admitted in sequence. Definitely rejected or stale attempts
do not consume a ticket.

Every open, recovery and operation validates metadata and table counts. Strict
opens and cache misses also scan value lengths and the contiguous retained
receipt range; cached operations reuse the exact previously verified state.
Exact recovery still reads its receipt. Missing or inconsistent evidence fails
closed. Value length is checked against the caller's read cap before allocating
the returned buffer. Changes, staged payload bytes, live objects, live payload
bytes and retained receipt counts have explicit persisted bounds. Live-capacity
checks apply to the final transaction contents, so a replacement at capacity can
insert and delete in either order. A rejected batch leaves every prior value and
receipt unchanged.

Ordinary writers can open with
`owner.open(namespace, incarnation, limits)?.reserve_capacity(RedbReserve { objects, bytes, receipts })?`
to leave part of each hard live-object, live-byte and receipt budget for management.
The reserve is scoped to that backend lease and does not alter persisted limits.
A new commit exceeding the resulting admission budget is definitely rejected
with `OperationRejection::ReservedCapacity`, without changing data or consuming
its ticket. Checks use the final transaction contents. Reads, exact committed
retries, recovery and receipt retirement remain available even after management
has used the reserve. Opening without a reserve can use the full hard budget.
The caller sizes the reserve for its bounded management sequence and applies it
to every ordinary writer; disk capacity and per-transaction bounds remain separate.

Any commit error is conservatively uncertain at the coordinator boundary. Call
`Coordinator::recover` before doing more work; if redb itself requires reopening,
close the owner and reopen the same database, namespace and incarnation. Never
regenerate the ciphertext or allocate a replacement ticket to resolve an unknown
commit.

`RedbMaintenanceRequest::FinalizeThrough(ticket)` is the trusted caller's attestation that
all recovery obligations through that ticket have finished. One transaction
deletes those local receipts and advances a durable retired prefix without
changing application values or revision. Retired tickets remain consumed after
reopening. This is neither a remote delivery receipt nor erasure of old MLS keys.
Recovery after an uncertain finalization returns the verified persisted prefix;
the caller may retry the same requested prefix if it did not advance.

## One-time development enrollment

`export_enrollment` captures validated namespace state and receipt evidence for an
explicit pre-enrolled development fixture. `import_enrollment` validates limits,
incarnation, all values and receipt continuity inside one immediate transaction,
and refuses any existing table in the target namespace. An invalid import leaves
no partial namespace. Unrelated application tables remain intact. The group
runner additionally binds the fixture configuration and local Reticulum identity,
and opens current state on subsequent starts instead of importing again.

These plaintext snapshots contain initial MLS secrets and are not production
backup/restore APIs. Installing the same endpoint snapshot twice forks its state;
retaining it also retains initial ratchet secrets. See the
[development enrollment guide](../../development.md).

## Limits and remaining work

The bounds cover logical payloads, not redb pages, cache, transient transaction
copies or total file size. The database owner must choose cache and disk budgets,
handle disk-full errors and schedule shared-file reclamation. Strict uncached
operations scan the bounded live-value and receipt tables. Attachment cache hits
avoid those scans; misses and explicit inspections retain that cost. Measure both
inspected work and chosen cache capacity before selecting large limits. The adapter does
not compact or replace the application's database. Open read transactions and
backups can retain old pages; removing a value does not scrub these copies.

Plaintext MLS keys can remain in database pages, cache, filesystem snapshots and
backups. Before deployment with real secrets, qualify a platform wrapping-key and
record-encryption adapter, locked-device behavior, key retirement, backup/restore
and account-reset policies. Neither a store incarnation nor redb detects rollback
to an earlier complete valid backup. Host tests do not qualify iOS/Android file
protection, abrupt electrical power loss or filesystem/device durability.

## Verification

The host suite reopens actual redb files, checks atomic crypto/outbox/projection
state, exact retries, ordered tickets, receipt retirement, capacity, shared app
tables, missing/corrupt evidence and lost commit responses. A parent process
kills a child immediately before and after the actual commit and receipt
retirement commits, then reopens the database and checks all-or-none state and
receipt fences. These four barriers exist only in the unit-test binary. They
test process death, not interruption inside `fsync` or a physical power cut.

The ignored `crash_child` test is a subprocess helper; the normal parent test
invokes it four times. A compile-fail doctest checks the exclusive owner borrow.

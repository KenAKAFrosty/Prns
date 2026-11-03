# Generic group records

`Client` publishes application-defined inline records on the same serialized
storage lane, MLS transaction and Reticulum runner used by text messages. It
creates no node, database or executor. `PublicationIntent::text` is an optional
pure encoder; the execution path treats its output as ordinary opaque bytes.

## API

- `admit_execute(Request, ExecutionContext)` synchronously admits every mutation
  and returns an `Outcome` waiter; `execute` is its async counterpart. Dropping
  the waiter does not cancel admitted work. Publications return `Published(id)`.
- `Intent::Publish(PublicationIntent)` supplies group, namespace/schema, opaque
  body and `Targets::Explicit` or `OtherMembers`. Explicit endpoints must be
  strictly sorted and unique. Targets select delivery obligations,
  **not confidentiality**: all MLS readers can decrypt. Applications assign their
  own namespace/schema; the carrier does not interpret their payload.
- Keep the request ID and captured `OriginalScope::Control` with immutable input.
  The scope contains the original control and exact local store incarnation, so
  recovery selects one retained namespace without searching other memberships.
  Exact retained retries compare original control, namespace, schema, body and
  original resolved targets, returning the same record without encryption or a
  change event. `OtherMembers` resolves only for new work after the control fence;
  it never resolves against a changed roster during recovery. A matching explicit
  target list is the same committed obligation. The execution clock may change,
  including to Unknown on exact recovery. Retention and retry horizon still come
  from the enrolled native profile. Cross-domain/other-group ID uniqueness is the
  caller's responsibility; this direct API adds no global request registry.
- `records` pages retained records newest-first across every profile. `record`
  retrieves one record. Both expose authenticated metadata, original targets,
  explicit membership incarnation, local insertion position, authenticated author
  identity and device delivery evidence. `record` and `delivery_progress` require
  that incarnation, so a record ID cannot silently select a different store. The service retains a
  record's Published/Received change until that record is retired in the same
  maintenance transaction; projections validate this invariant. Text history filters
  and decodes the text profile over this same projection.
- `changes` reads the durable bounded feed. Its `ChangeToken` binds group,
  storage incarnation and position. `None` starts the current membership feed;
  a supplied token selects that exact retained membership, including an archive.
  Persist that token with an application projection; unknown/purged or wrong-group
  incarnations are rejected rather than rebound to the current store. `history_pruned` means
  the cursor crossed retired events or a maintenance invalidation gap. Rebuild
  from retained records even when the returned event list is empty.
  This is not an eternal audit log or an exactly-once external effect protocol.
- `delivery_progress` reconstructs authenticated receipt counts, pending/expired
  targets and membership interruption from committed state. Missing retired
  records return `None`, never successful delivery. Device receipt does not mean
  a human read the text or an actuator executed a command.

Before queueing, the owning `InputBudget` checks fixed job bytes and retained
dynamic allocation capacities against its selected `ClientLimits`. Every client
on that lane shares the allowance; a dropped waiter does not release accepted
input before execution ends. A single oversized job returns `InvalidInput`;
temporarily exhausted aggregate input capacity returns `Busy`. The enrolled
group's canonical profile separately limits content and targets at the service
boundary after exact recovery. Unsorted/duplicate explicit targets return
`InvalidInput`, and empty targets return `NoRecipients`. Actual storage capacity
exhaustion remains distinguishable. Incompatible groups and invalid page sizes
are rejected. Caller-owned unpolled input and returned pages are outside the
accepted-job allocation budget.
The owning group library provides indexed storage, paged queries and fair work.
Selected profiles and local resource budgets govern target admission; there is
no independent eight-target client ceiling. Host measurements apply only to
their tested workload and resource selection.
Errors retain their typed capacity, retry, clock, storage and protocol variants;
`Unavailable` or a dropped result never authorizes minting a replacement request
ID. A new retry token is created only for a new application operation.

The generic core and text codec remain allocation-free `no_std`. This native
API owns vectors on its caller's native lane; the existing MLS/service layers
remain bounded `no_std + alloc`. The API does not add a requirement for a native
executor or database to the portable core.

## Retained memberships and bounded paging

`records` traverses current and archived memberships newest-incarnation-first,
then descending local insertion positions. `messages` uses that same traversal,
filters the text profile and decodes the generic record projection. Its author
mapping and delivery interruption come from the same authenticated generic facts.
No numeric position or summed revision orders unrelated incarnations.

Each query visits at most four namespaces, including any explicit control-fence
validation. It scans at most the persisted `max_changes` for each record namespace.
The persisted catalog byte budget bounds descriptor decoding and total retained
metadata; retained-incarnation traversal is independent of the current-group
entry quota. A page may have **zero rows and a continuation**, including when every visited record uses
another content profile. Keep following `next_before` until it is `None`.

Page row limits are not a caller-selected output-byte budget. Payload size comes
from the enrolled profile, and decoded text and retained display pages add their
own allocations. Consumers should select page sizes for their payloads and bound
their displayed cache. Larger payload profiles may justify an additional canonical
byte/work budget; the current API does not promise a process-memory ceiling.

`StoredRecord.position` and `Message.position` reuse `ChangeToken` to identify
insertion provenance. `RecordCursor` carries group, incarnation and an optional
exclusive insertion boundary; `None` starts that incarnation. Convert a record's
position with `RecordCursor::from(position)`, or use the page's continuation.
An expired boundary record does not invalidate the position while its incarnation
remains retained. Wrong-group, unknown/purged incarnation and future positions
are rejected. A continuation issued before readmission keeps selecting its old
source; restart from `None` to include a newer membership. The fixed-size `PGH1`
encoding preserves all 64 position bits and is a local cursor, not an authenticated
network claim or a database schema migration.

`visited_heads` contains only the namespaces visited by that page, including
empty ones. Its maximum length is four. `current_incarnation` identifies the
catalog's current membership when queried, even when the page reads older history.
These observations do not create an atomic snapshot or extend record retention.
Close leaves retained history; DeleteLocal purges its owned namespaces and makes
their old cursors unusable. Without the separately selected local archive, this
access does not preserve records beyond protocol expiry.

## Rebuilding a consumer projection

Record pages are live queries. Start from `None`, remember `current_incarnation`,
and capture each source's first `visited_heads` token before continuing through
its records. Stage a replacement projection keyed by `(incarnation, record ID)`.
After paging, replay each source's changes after its captured head, using that
same incarnation for record lookups. Never replace an earlier captured head with
a later page's head: that could skip insertions during paging.

If `current_incarnation` changes between pages, or a final current-feed observation
reveals a newer incarnation, restart the traversal. A source that disappears or
reports `history_pruned` also requires a fresh bounded rebuild. Consumers choose
finite page/retry/cache budgets; exhausted budgets report incomplete reconstruction
and preserve the previous projection and cursors. This API supplies neither a
cross-incarnation atomic snapshot nor an exactly-once external effect protocol.

Expiry maintenance removes records and associated feed entries **and advances the
feed head in the same transaction**. The new position has no event: its gap makes
`history_pruned` true for a consumer crossing it, including one already caught up
before cleanup. It requires no unbounded deletion log. Maintenance that only
retires internal recovery evidence may conservatively invalidate a projection
too. An exact maintenance retry does not advance the head again.

Rebuild into a replacement projection, so records absent from retained pages are
removed; upserting into an old projection alone would keep deleted entries. Save
the rebuilt projection and cursor together only after replay succeeds. On an
error or exhausted rebuild budget, keep the previous cursor/projection and report
incomplete refresh. A page can lose a record before a subsequent read; `record`
returning `None` is authoritative at that query and must remove that projected ID.
Continue polling the feed after a successful refresh: no live read is a promise
of permanent retention. This contract does not provide an atomic historical
snapshot, individual deletion events, or exactly-once external effects.

## Telemetry example and recovery evidence

Run from the repository root:

```sh
CARGO_INCREMENTAL=0 cargo run --locked --manifest-path prns-groups/Cargo.toml \
  -p prns-groups --example telemetry
```

The example encodes an eight-byte sensor reading, publishes it through the
generic API, consumes the durable change feed at a second independently
credentialed endpoint, and waits for its authenticated receipt. Its in-process
controlled carrier still traverses real Reticulum nodes, Links, requests and
Resources. Fixture enrollment and temporary plaintext native stores are demo
setup, not production provisioning or protected storage.

Its deliberately single-incarnation `examples/support/telemetry_projection.rs`
consumer rejects readmission or additional retained incarnations. It stages a
replacement map and cursor, captures the feed head before paging, and retries invalidated
rebuilds at most three times. Each pass is limited to 32 pages of 16 items and a
caller-selected cache capacity capped at 256 readings. Exhausting a bound returns
`Incomplete` and keeps the previous map and cursor; errors also leave them intact.
This helper is native application code, not an allocation requirement on the core.
The demo also runs expiry maintenance and verifies that the consumer removes the
expired reading after observing the durable invalidation.

`generic_records` tests lost-result admission, database reopen before delivery,
exact retry/conflict checks, scoped/pruned change cursors, mixed telemetry/text
pagination and receipt convergence between independent Reticulum nodes. They also
cover caught-up consumers after a lost maintenance result and database reopen,
unchanged cache/cursor on incomplete refresh, and deterministic insertion or
retirement between record paging and feed replay, including exhausted rebuild
attempts under repeated retirement. Service `maintenance_recovery`
tests prove that deletion and the invalidation gap survive the same crash cuts,
and exact maintenance replay does not advance the head twice. Existing
`lane_recovery` and session checkpoint tests also exercise abrupt process exit
at durable send/receive/receipt boundaries. These establish host recovery, not
phone suspension, physical power-loss behavior or protected at-rest storage.

The runtime-free isolated consumer additionally exercises opaque retained history
across repeated Leave/readmission, an empty bounded page with continuation,
readmission during paging, explicit-incarnation lookup, original-control exact
retries, reopen and DeleteLocal. Native tests exercise the same empty-page behavior
for text filtering and reject malformed or noncanonical cursor encodings.

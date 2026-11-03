# Group ownership and scheduling

`prns-groups` supplies local generic records and authenticated
enrollment, with optional text projections and Reticulum synchronization. See
[authenticated enrollment](enrollment.md) for creation, invitations, joining,
security boundaries and recovery. Storage jobs run on an explicit serialized
lane: attach to the caller's existing owner, or construct the standalone local
owner below. Neither path creates a node or async executor.
The core and [text profile](profiles/text/README.md) are allocation-free
`no_std`; MLS and the generic service use `no_std + alloc` with explicit storage
and admission limits. See the [MLS allocation boundary](security/mls-rs/README.md#what-is-bounded).

The optional foreign binding workspace generates the
canonical local client directly and supplies a narrow key-owning factory. Public
client operations return the core `ApiError`, preserving rejection/retry reasons
and separating uncertain outcomes. Lower-level manager/storage APIs retain
backend diagnostics. Record/change page limits are checked `u32` values (1–100);
public receipt and membership counts use `u64`.

## Features and construction

With `default-features = false`, local fixed and managed groups work without a
native host or ambient runtime. The command interface uses Tokio synchronization
primitives only; an ordinary Rust future executor can wait for responses. The
`text` feature adds the pure text encoder and message projections. The `network`
feature adds the borrowed native host adapter and synchronization futures;
`headless` enables both plus the development peer's owned runtime and storage lane.
Defaults are `text` and `network`; applications that need the local boundary
must disable defaults explicitly.

`owned::LocalOwner` is the standalone file-backed construction path. Its
`create_new(path, identity, options)` creates a new private redb file and managed
catalog; `open_existing` requires the existing catalog and exact expected
identity. `open_fixed(path, provisioned, identity, options)` validates an already installed
fixed membership and creates no enrollment schema. None installs development
fixtures. All return the ordinary `Client` from `client()`; there is no parallel
command facade. Construction blocks while its worker opens and validates state,
so perform construction off a UI thread.

`LocalOptions` requires an explicit queue capacity (1–1024 pending jobs, plus the
executing job) and cache budget (1 MiB–1 GiB). These bound the queue and redb cache,
not all process memory or filesystem use. The immediate parent must exist as a
private directory controlled by the embedding. The Unix implementation creates
files with mode 0600, rejects final-component symlinks, multiple hard links and shared permissions,
and requires a private immediate parent. Other platforms return
`UnsupportedPlatform` until their private-file and durability contracts are
implemented. Keep ancestor directories under trusted control. Paths resolve
before worker dispatch so later process-directory changes cannot retarget them.

The managed catalog uses indexed rows and bounded pages. Stored conversation
count, declared per-group member capacity and the local working set are independent
selections; the library imposes no replacement global conversation-count ceiling.
Resource admission is still bounded by the chosen catalog, namespace and queue
policies. The [opt-in workload tasks](#opt-in-workload-measurements) describe
representative and larger measurements without turning test sizes into supported
limits. Embedded fixtures use their own selected profiles.

Standalone owners may opt into
`.with_validation_cache_entries(NonZeroUsize)` to retain bounded namespace
validation evidence across reads and confirmed writes. This working set is
separate from redb's page-cache bytes and does not cap stored conversations.
Both managed and fixed construction still validate cold state strictly; an
uncertain write discards the affected proof. Closing drains accepted jobs and
releases the proof cache before reporting physical database cleanup. Without
this option, standalone backend reads retain strict full validation.

`Client::enrollment_work` is a read-only bounded selection, including empty
continuation pages. The owning runner must schedule `Client::maintain` explicitly
for admitted recovery, expiry, terminal fanout and cleanup; maintenance runs
independently of the number of pages read. The production runner already does so.

Construct options with `LocalOptions::new(queue_capacity, cache_bytes)`. Fresh
managed stores select `.with_catalog_limits(enrollment::CatalogLimits { ... })`.
The default has no group count quota (`max_groups: None`) and selects 2 MiB of
logical catalog bytes. An embedding may choose an explicit count quota and its
own byte, row, transaction and maintenance budgets. The nested `namespace` field
uses the storage owner's canonical `RedbLimits`. Prior membership incarnations
share their selected budgets and are never evicted to admit new work.
The selection is committed atomically in indexed catalog v5, and reopening reads it from
storage even when caller options differ. Older catalog versions, missing limits,
unsupported selections and partial state are rejected intact; opening does not
migrate or reset them. Conversation and invitation queries return bounded live
pages; exact conversation detail and separately paged members do not require
loading every stored group or roster.

`Intent::Create { title, profile }` retains an explicit canonical `GroupProfile`.
The library default is a selectable eight-member policy, not an enforced native
ceiling. Offers, Join digests and signed controls pin the selected member, record,
MLS and enrollment-envelope bounds. Unsupported peers return an explicit
`UnsupportedProfile` invitation result. The owner verifies the actual encoded
envelope bound, including base64 and both signed rosters. A validated profile is
a compatibility and admission policy, not a measurement of a particular host or
phone's performance at the selected member ceiling.
`resolve_group_profile` derives plaintext and enrollment encoding bounds from a
requested member ceiling and explicit record, wire and snapshot byte policies.
It uses the owning codecs and checked arithmetic; it neither allocates a roster
of that size nor promises that all possible memberships fit those local budgets.

`LocalOptions::with_client_limits(ClientLimits)` selects one shared input budget
per worker lane. Defaults permit 1 MiB per job and 8 MiB across accepted jobs.
Every `Submit` implementation retains one `InputBudget`; independently constructed
clients and scoped views share it. Accounting includes owned vector/string
capacity and the job itself, and survives cancellation until the owner finishes
or fails the job. This excludes caller-owned unpolled async input, query results,
executor scratch allocations, redb cache and disk usage. Group-specific content
limits remain enforced by the service after exact retained recovery.
`InputBudget::limits()` exposes that same immutable policy, and
`ClientLimits::required_job_bytes` includes the owning Job representation when
checking a carrier's full input payload budget.

An embedding can retain `Attachment::open_cached` with an explicit validation
working-set size. It caches namespace validation evidence against the exact
physical database allocation, without retaining read transactions or setting a
stored-group ceiling. Drop the attachment before reporting physical close.
`Attachment::set_profile_support` validates stored group and invitation profiles
against carrier-projected payload bounds before network readiness. This local
support is not persisted as group authority; failed checks leave it unchanged.

Catalog bytes fund invitation/API evidence and admitted cleanup obligations;
their capacities are not derived from the number of stored groups. The default
unsolicited inbox remains four offers total and two per authenticated peer,
selected independently through `CatalogLimits`. Completed evidence remains until
its replay horizon, so dismissal need not reclaim space immediately. Admission
reserves the encoded bytes required for terminal proofs, notification work and
local deletion. Ordinary writes cannot spend those reservations.

Local Delete immediately hides the selected history generations and then purges
their exact namespaces in bounded maintenance steps. Replay fences and unresolved
departure/terminal delivery remain independently retained. Reaching a byte or
operation budget reports capacity rather than silently discarding history.
Use representative workloads to measure
paging, joins, idle work and storage costs, with explicit extrapolation to the
design targets. Full-size stress runs are optional; host measurements do not
establish device-specific latency or radio behavior.

The optional
`archive` feature adds `.with_archive(archive::Limits { max_records, max_bytes })`
for fresh creation. Selection and identity-wide logical budgets commit with the
catalog. Reopen uses the persisted selection and limits; it never backfills an
existing store or creates a missing archive. A binary without archive support
rejects a selected archive before owner readiness.

When selected, ordinary `records`, `record`, `delivery_progress`,
`delivery_targets`, and the optional text decoder read local history until
explicit local deletion. Protocol expiry
still removes delivery work and its bounded change feed. Archive admission rejects
new records atomically when either budget is full; it never evicts history.
Receipt updates use preallocated row space, and Close/local Delete remain possible.
A filtered page may be empty with a continuation because each page bounds inspected
rows as well as returned records. Archived receipt flags preserve only the durable
target evidence actually observed: receiving a record proves local durability,
not delivery to its other targets. Budgets exclude redb page overhead; deleting
rows neither promises immediate file shrinkage nor secure media erasure.


Creation failure can leave a partial file or a fully committed catalog whose
reply was lost. Preserve it and try `open_existing` with the same identity. A
missing schema, empty file, corrupt catalog, unknown version or wrong identity
fails; reopening never falls back to creation. A valid redb file may undergo
redb's normal transaction recovery. Startup confirms file and parent-directory
synchronization before publishing readiness. A remaining incomplete creation
requires inspection; this API does not reset it automatically.

`LocalOwner::close(&self)` fences admission synchronously and returns the shared
storage worker's cloneable `Completion<OpenError>` future. It observes accepted
jobs draining and the manager/database releasing on that same worker; repeated
calls and dropped waiters preserve the result. Dropping the owner initiates the
same fence without blocking finalization. Keep or await completion when another
owner must reopen the file. Cleanup completion does not assert that each domain
job succeeded, join the OS thread, or stop a separately supplied host. No cleanup
thread or ambient runtime is created by close.

Ordinary Rust callers supply an `IdentityHash`. Trusted native adapters can
instead pass `LocalIdentity::retain(hash, guard)` after deriving the hash from
their key capability. The worker retains that opaque guard until after its
manager and database drop, including owner finalization and startup failure.
Fixed construction checks the supplied identity against provisioning before any
file access. This is lifetime retention, not private-key validation; foreign
adapters must not accept an arbitrary caller hash as proof of a key capability.
The group library has no dependency on the identity catalog or key owner.

`GroupManager::open_fixed(database, provisioned)` attaches an authenticated,
previously installed fixed membership and validates its configuration without
creating an enrollment catalog. Managed operations are unavailable in that mode.
`GroupManager::create_managed(database, identity)` explicitly creates a new
managed catalog and rejects existing/partial state. `open_managed` validates an
existing catalog without writes or initialization. A missing configured catalog
is an error. `Enrollment::initialize_in` supports a caller-owned transaction that
atomically binds the identity and selected domain schemas; its caller must commit
with immediate durability before readiness. It opens no nested transaction.
Both modes execute the same record engine. `StoreSession::open` remains the
lower-level single-group attachment for an already provisioned namespace.

On Unix, the isolated consumer fixture constructs the production `LocalOwner`
for offline managed invite/join/lifecycle and opaque record publication/query/reopen,
plus fixed publication/reopen without an enrollment catalog. Its fixed test
explicitly installs development provisioning before attaching the owner; reopen
uses the retained installed configuration without running the fixture importer. Its dependency check rejects native host/runtime adapters and
the text profile, and requires Tokio's `sync` feature alone. Run it from this
repository with:

```sh
python3 prns-groups/tests/check-local-consumer.py
```

The [generic record API](generic-records.md) supplies publication, retained record
queries, durable changes and delivery progress beneath text helpers. The
[shared synchronization progress API](progress.md) exposes bounded scheduling
observations without confusing them with authenticated device receipts.

## Ownership and recovery

The database owner holds `GroupManager` (or a single-group `StoreSession`),
accepts bounded `Job`s through `Submit`,
and executes each to completion. `Client` is a borrowed command interface; it has
no authority to stop the node. Every mutation uses the same `Request`:
`RequestId`, an `Intent`, and its captured `OriginalScope`. `ExecutionContext`
supplies the trusted platform clock separately; refreshing that evidence does
not change the immutable intent. `admit_execute(request, context)` validates and
queues synchronously under the caller's lifecycle lease; `execute` awaits it.
Dropping the returned waiter cannot cancel admitted work. The storage owner wakes
scheduling after a committed outcome even when that waiter is gone.

`Intent` covers generic publication, creation, invitation, response, cancellation,
dismissal, member removal, leave, close and local deletion. `Outcome::Published`,
`Created`, `Invited` and `Saved` describe local committed results, never remote
completion. Text only encodes a `PublicationIntent` with `Targets::OtherMembers`;
the same generic executor can recover those encoded bytes without the text
feature. `GroupManager::execute_request` is the synchronous on-lane seam, so a
retained facility need not submit reentrant work through its own async client.

Before constructing a direct group mutation, read `group_scope(group)` and keep
that value with the request. Joined operations require
`OriginalScope::Control { control, incarnation }`, capturing both the complete
`ControlReference` and exact local `StoreIncarnation`; local deletion can also capture an
exact `Unjoined(StoreIncarnation)`. `None` is reserved for creation and stable
invitation-ID actions. `Client::at_control` scopes reads and carrier work;
mutation authority always comes from the request itself. Exact results recover
from the exact captured namespace through its incarnation index before checking
current scope or execution time. Recovery never scans other memberships. Unknown time permits that recovery, but not new work. An
unsent request with a stale scope fails; an old successful deletion retry cannot
purge a replacement membership.

`create_request_id` returns a semantic `RequestId`: `PGR1`, an eight-byte big-endian
creation time and 20 bytes of OS entropy. Callers must use unique IDs and retain
unchanged requests after ambiguous results. Receipt conflicts are checked in the
existing domain scope: publication results across that group's retained
incarnations, and management results across the managed catalog. The independent
direct lane needs no owner-wide request registry or retained-intent journal.
The catalog-authorized native lane described below selects retained ownership.
Lower-domain publication receipts compare original control, namespace, schema,
body and resolved targets; matching
`Explicit` and `OtherMembers` selectors describe the same delivery obligation.
Management fingerprints include the captured original scope. These are local API
receipts; network frames and MLS control formats are unchanged. Old group-scoped
local mutation fingerprints are not migrated to the new request contract.

Expired or unknown legacy IDs remain rejected by the existing retry policy;
callers must not silently mint replacements. Exact retained opaque IDs remain
recoverable. Explicit local deletion removes retained publication results.

## Change observation

Call `Client::subscribe` before the initial snapshot, then use
`GroupObservation::next` to wait for possible change and refresh ordinary queries
or durable record/change cursors. A subscription has one coalescing hint, not an
event queue or delivery receipt. Late registration does not replay past hints.
The owner publishes before returning a job's reply, including when the caller has
dropped that reply or an error follows a possible durable write. Pure unchanged
reads do not manufacture notifications.

Each attachment supports at most 32 live subscriptions, with one pending `next`
per subscription; exceeding either bound returns `Busy`. Cancelling a pending
wait releases its waiter and preserves an unconsumed hint. Dropping the domain
attachment closes subscriptions with `Unavailable`; this alone is not physical
file cleanup confirmation. A reopened owner needs a new subscription and
snapshot. User wakers run outside subscriber locks, and a panicking waker does
not strand sibling subscriptions or unwind the storage owner.

A standalone manager owns its observation source. Embeddings that reconstruct
short-lived manager views retain one `Attachment` beside the physical database
and use `GroupManager::open_attached(database, &mut attachment)` for each
sequential job. The attachment owns the source across those views and validates
identity plus store incarnation. It requires an explicitly selected durable
group stamp; ordinary standalone managers can observe without that schema.
`Enrollment::validate_in` and retained `inspect_in`/`validate_in` inspect the
caller's read transaction without initialization or nested snapshots. The caller
still owns exclusive access to the physical file and attachment.

The shared redb commit helper advances a selected durable group revision
in the same transaction and marks the local witness before the commit attempt.
Ordinary writes never initialize the revision schema. Its revision supports
bounded dependency scans; it is separate from a subscription hint and the
existing durable record/change cursors.

## Optional retained submission

`retained::initialize_in(transaction, identity, Limits)` explicitly selects a
fresh retained-operation schema in the caller's startup transaction. Commit it
with immediate durability before publishing readiness. Ordinary manager open
attaches and validates an existing schema; partial tables, wrong identity,
unknown versions or invalid rows fail instead of silently disabling retention.
Retention format v3 intentionally rejects older retention formats without migration.
Cold readiness validates rows and both reciprocal indexes in a streaming pass.
Persistent `Attachment` keeps that verified selection; lexical manager views
check the header, marker entries and table counts, then use exact keys or ranges.
This relies on the existing exclusive database-lane contract; arbitrary external
same-count row changes require a new full inspection.

`retained::Limits` independently selects total operations/reservation bytes, one
encoded row (`max_row_bytes`), one decoded owned row (`max_operation_bytes`) and
terminal prune work (`max_prune_rows`). These are local policies, not group or
membership ceilings. No 256-operation or 32-MiB upper cap is imposed. Failed
bounded pruning aborts the entire acceptance transaction, including its replay
floor; unresolved rows are never pruning candidates. Success atomically updates
the row, scope/terminal indexes, exact counts, reservation total and replay floor.

`operations(after, PageLimits)` uses RequestId order. The budget accounts for the
actual returned Vec capacity plus every dynamic Intent capacity. It must fit one
maximum selected owned operation; an incompatible budget returns a typed error.
A byte-limited page may return fewer rows. The final returned ID is the next cursor.
A query also needs a separately bounded one-row decode/lookahead; it does not
retain the whole operation history. Encoding is capped and avoids cloning input.
Admission round-trips the encoded row so decoder Vec growth cannot strand an
accepted operation. Transient JSON scratch is conservatively bounded by eight
times the encoded-row selection plus the fixed Operation representation, separate
from returned page memory. Allocator metadata, database caches and physical disk
usage are outside these logical bounds.

`StoreSession` rejects this schema: retained exclusions require `GroupManager`.
The standalone `LocalOwner` currently selects the managed catalog alone; the
retained schema is available through the existing borrowed manager integration.

`GroupManager::submit_retained(id, intent, context, authority)` captures original
membership scope once and commits the canonical `Request` before dispatch. It
returns an `Operation` with `Pending`, `Dispatched`, `Uncertain(ApiError)`,
`Completed(Outcome)` or `Rejected(RejectionReason)` state. Local completion does
not establish remote delivery. No alternate input codec, database owner or
runtime is introduced. `retained_operation` and bounded `retained_operations`
recover status; absence does not prove an effect never occurred.

`Client::submit` (or synchronous queue transfer through `admit_submit`),
`operation`, `operations` and `dismiss_operation` use those same declarations.
The assembly passes each opaque `Job` to `GroupManager::execute_authorized` with
its `admission::Authorization`. Classification stays in the group owner. A
service-mutation permit carries the full canonical request ID; an identity
adapter can derive its transient mutation identifier without inspecting Jobs.
The queue accepts a job before durable acceptance, so a pause between enqueue
and execution still prevents a new retained intent from being persisted.

The authorized boundary supports retained commands, exact direct execution and
audited history/configuration reads. `execute` keeps the caller's complete
`Request`, including `original_scope`, and uses the same retained coordinator as
`submit`; it never captures a replacement scope. A matching existing row goes
through that coordinator's recovery and re-admission. With no row, an exact
domain receipt recovers before fresh token checks. Otherwise the selected
retainer must durably accept the exact request before domain dispatch. A missing
retainer permits exact recovery only and rejects fresh work with `Configuration`.
Retained equality covers the full canonical request, including its target
selector; the lower-domain equivalence above does not weaken that binding.
`execute` returns the canonical outcome/rejection or uncertain cause; the
operation remains queryable by request ID after caller loss or an uncertain
reply. Managed record receive and authenticated enrollment frame/response
handling additionally require an attached manager and a carrier-capable
authority. Their existing canonical Client methods use the same owner boundary.
Plain `GroupManager::execute` continues the independent direct lane but rejects
retained commands; it never invents an unmanaged permit.

Record receive inspects the canonical stored result before requesting fresh
network authority. Fresh receive captures its exact namespace/incarnation and
next physical operation ticket. The embedding must check its original captured
network generation and retain the original permit plus key guard before
`acquire_carrier` returns. The group attachment immediately records one bounded
continuation; it retains no payload and creates no second intent journal.

An uncertain receive fences every other domain write, including maintenance and
receipt repair, while pure history remains available. The embedding calls
`Attachment::recover_carrier` before lifecycle progress, including during close.
Only exact physical committed/not-committed evidence or positive stage evidence
settles that original permit. A failed probe or failed settlement retains both
fences. Once the receive commits, the durable receipt intent owns any remaining
receipt repair, which can recover without fresh network admission. Process
restart uses the ordinary durable envelope result and receipt intent; live
permits and the bounded in-process continuation do not survive process death.

Enrollment handling runs maintenance, accepted-package recovery, and correlated
work lookup under Recovery before requesting fresh network permission. A fresh
raw effect has one physical commit: either a checked catalog projection or an
exact namespace ticket. Terminal retirement and its catalog projection enlist
in that same namespace transaction. No second journal or table is introduced.
The live slot retains only canonical catalog before/after digests or a namespace
ticket; ambiguous errors keep the original permit until an exact read proves
the outcome. A second physical effect cannot replace the first attempt's proof.
Completed work is an idle duplicate, and new raw frames still require the
captured network generation. Encoded frame/response sizes are checked before
queue admission using the protocol's existing frame bound.

The supplied `Authorization` must serialize pause/close against actual dispatch.
Its owned permits settle explicitly; dropping a permit does not manufacture
settlement. Recovery checks run before fresh mutation acquisition. A committed
receipt can complete while paused, whereas missing evidence needs fresh
local/service mutation authority. A failed recovery lookup preserves uncertainty
and stops that attempt. `Retained` settlement releases only the finished live
execution: the durable unresolved row still owns recovery and group exclusion.
`Unmanaged` is explicit authority for embeddings without an identity catalog;
catalog-managed assembly must use its real tracked admission adapter. This
lower-level seam and its tests do not yet qualify that production integration.

Unresolved operations exclude conflicting mutations in the same group, including
direct execution; unresolved creation uses a separate creation scope. An exact
direct call cannot bypass the retained dispatch markers to create an effect.
The authorized lane resumes that row through the shared coordinator; the
independent direct lane only recovers its existing domain effect. Completed/rejected results do not block subsequent
work. `dismiss_retained` removes terminal results only. Both explicit dismissal
and bounded automatic terminal pruning atomically advance an inclusive request
retirement floor. Unresolved rows are never evicted. At full capacity, another
request issued in the same second as the only evictable result can be rejected by
that floor; retain its ID and treat the rejection according to the retry policy.

Retention limits select the operation count, reserved encoded bytes, encoded row
size, returned operation bytes and candidates pruned per acceptance. Defaults are
64 operations, 8 MiB of reservations, 128 KiB per encoded/returned operation and
32 prune candidates. These are selected logical budgets, not fixed library-wide
count ceilings or physical redb file limits. Reopening never refreshes an
original membership binding or replaces an uncertain request ID.

With `network`, the single-group `run` is a caller-owned future. It polls peers with bounded
requests/bytes and backoff, persists inventory cursors, repairs pending device receipts and serves
incoming requests concurrently. Every database operation finishes before the next
network wait, allowing reciprocal synchronization to progress without holding a
service lock. Cursors are performance checkpoints; losing one safely rescans
durable deduplicated envelopes. A full database lane delays configuration and
checkpoint work with bounded retry intervals; it does not terminate the runner.
The caller can stop during startup or retry waits. Stop/drop closes owned group
links, not the shared host. The composition must fence new admission and drain accepted store jobs before
destroying or resetting its database.

`run_multi` serves enrollment and multiple groups on the same borrowed host.
Before startup, it installs the transport's checked ciphertext/enrollment payload
ceilings on the existing owner lane. Subsequent Create and enrollment use that
same support, including other clients and lexical manager views. The single-group
runner checks its actual ciphertext capacity too. Multiple runners can only
tighten support during one owner lifetime; a wider attempt cannot undo an earlier
bound, and a failed tighter attempt leaves it intact. Cancellation after admission
does not undo the owner job. Reopening the owner selects local support afresh.
`run_multi_scoped_with_announcements` accepts a `HostServiceScope` and verifies
the store identity against it, allowing multiple independent identities to share
one host. Its sync/enrollment transports authenticate using that explicit local
identity. The caller still owns catalog admission, scoped storage and the complete
Reticulum retirement barrier; stopping the runner alone is not that barrier.
Enrollment and content admission alternate with round-robin peer selection.
Two caller-owned outbound futures can progress concurrently, with at most one
active attempt per physical identity across all groups and enrollment. One
unavailable peer cannot fill both slots. Incoming enrollment and content requests
progress independently. No additional worker or executor is created.
Storage calls are fenced to the locally committed control head. Established
members can synchronize using cached membership while the owner is offline;
this is not a global membership-freshness guarantee.

Both runners use the allocation-free core's `SyncPolicy`. The carrier reads the
selected interface's configured bitrate, route hops and measured Link RTT.
Unknown or routed paths remain conservative even with a fast local Bluetooth
hop. Small constrained batches and bounded, byte-aware waits replace the former
ten-second whole-pass cutoff. `run_multi_with_policy` supplies caller-selected
policy; `run_multi` uses defaults. The single-group runner also preserves its caller's
hard work budgets and timing floors.

Default fast direct paths allow up to four envelopes per pass and two-second
active spacing; constrained paths allow one envelope and fifteen-second spacing.
Idle spacing backs off to ten seconds on healthy fast direct paths and five
minutes on constrained/unknown paths. Failures back off to five minutes on either.
Positive jitter spreads recovery contacts. Fresh durable work can shorten an
idle or failure delay to the contact-spacing floor without resetting failure
history or opening another active attempt for the same peer. Unchanged membership replies remain idle. The multi-group runner announces its two fixed
service destinations every six hours plus jitter after startup, following the
[announce strategy](transport/reticulum/docs/announce-strategy.md).

For live preferences and manual announcements, create `announce_control(interval)`
and pass its request receiver to `run_multi_with_announcements`; retain the
returned `AnnounceControl` in the application owner. `set_interval(None)` disables
automatic announcements, including startup. Changing an interval does not
announce immediately. `announce().await` requests both existing destinations
through the same bounded scheduler and returns local admission, cooldown, busy
or unavailable status. Startup, periodic and manual attempts share a 60-second
cooldown, including failed/uncertain attempts. The caller owns preference
persistence. The cooldown is per runner lifetime and is not a durable
restart-storm fence. An identity-service embedding can disable these per-runner
timers and use the [durable shared scheduler](../prns-identities/runtime/README.md#shared-announce-scheduling)
across its service sets; selecting and driving it is a separate composition
responsibility.

Both runners share prompt delivery over negotiated group Submit requests. A
bounded scan of durable authored-record obligations selects exact retained MLS
ciphertext for each intended peer. The receiver commits the record and its MLS
receipt before returning that exact receipt on the submitting Link; the sender
validates and stores the receipt before updating delivery. Pull-created receipts
also return on the existing fetch Link when the request/byte budget permits.
Transport response admission alone never means durable device delivery.

There is no second durable message queue: process-local scheduling holds only
inventory positions, backoff and a temporary capability observation. Restart
reconstructs pending work from records and receipts. New records take precedence
over retry rotation; old unacknowledged records rotate fairly. After at most four
completed Submit exchanges for a peer/group, the next eligible attempt performs inventory
repair even if authored work remains. Incoming requests and active outgoing
Links continue progressing while storage queries wait. Both modes retain two
outgoing slots; multi-group admission also serializes each physical peer across
enrollment and all its groups.

An unsupported or unanswered read-only capability probe temporarily selects
legacy pull for five minutes before probing again. A legal envelope that cannot
fit Submit's larger framing overhead also falls back to pull. Successful push
returns promptly without opening a second Link for an unnecessary poll. A new
exchange still establishes a new Link; a persistent cross-exchange Link pool is
not implemented. Failed delivery retains its obligation and uses normal backoff.
No per-message announce is sent. These bounds do not establish a LoRa duty-cycle
guarantee or retain knowledge of downstream bottlenecks beyond current path
observations. See the [isolated LoRa trial procedure](transport/reticulum/docs/lora-qualification.md).

Completed inventory scans retain their covered head. An unchanged poll needs
only a snapshot request and does not rewrite the identical stored checkpoint.
Both record and receipt passes scan only a new suffix. Changed remote generation
or regressed head falls back to a full scan. Data added beyond a frozen head
waits for a later snapshot; the idle bound is not a delivery-latency bound.

Native cursor rows bind progress to the local control head, authorization era
and store incarnation in addition to the existing namespace/peer key. A changed
context invalidates coverage without accumulating old rows. A bare transport
checkpoint has no enrollment binding and triggers a conservative rescan. Cursor
rows are disposable; discarding one never authorizes deleting group records or
keys. Accepted records, receipt
repair/output and cursor persistence retain their transaction order. Ordinary
expiry preserves remote coverage; arbitrary deletion of live local records
would need explicit repair.

`Client::maintain(clock)` runs one bounded pass on the existing storage lane.
The sync loops invoke it, and send/receive admission performs a pass before new
work. Each group pass selects at most one expired record and four service
operations. The initial text policy retains protocol content for 14 days;
expiry may remove undelivered protocol work, but never fabricates a delivery
receipt. A selected archive preserves its generic plaintext history and recorded
delivery evidence until explicit local deletion; its capacity is a separate
caller-selected bound. Cleanup
advances a durable time floor, removes record/retry/output facts atomically,
and invalidates inventory snapshots. The oldest pinned operation is prioritized
when eligible, allowing the native receipt prefix to advance. New work reserves
two backend receipt slots for cleanup; the final slot may only be spent when
retiring the oldest outstanding output. Existing completely exhausted stores
may require a separate recovery procedure; an upgrade cannot invent free slots.

`StorageFull` preserves accepted work and the caller's immutable request. Retry
after maintenance frees capacity, using the same ID and body. Limits constrain
concurrent retained work rather than lifetime sends. The 14-day request fence
prevents retired sends from becoming new publications after clock rollback.
These policies reclaim logical objects and reusable database pages; they do not
shrink the redb file, reserve filesystem space, encrypt phone storage or guarantee
physical erasure of old secrets. The core retry-token policy remains allocation-free.

Messages have durable insertion positions within each membership incarnation
and receipt projections. Queries return bounded pages with source-qualified
continuations and the visited source heads; there is no aggregate history revision. Callbacks and polling can
refresh projections; they are not the only record of changes. Delivery targets
exclude the publishing endpoint. All members remain MLS readers.

`message(group, incarnation, record, clock)` applies the optional canonical text
projection to an exact retained record. Missing records and non-text records
return `None`. It uses the same author timestamp and delivery counts as text
history; it does not invent a local arrival time.

`delivery_targets(group, incarnation, record, after_endpoint, limits)` reads the
original targets of that exact record. `None` means the record is absent; a
present page can be empty after its last target. Endpoint order is stable, and
`next` is the exclusive continuation. Returned allocations obey `PageLimits`;
each call decodes one record and its flags under the selected profile bounds,
with work bounded by that record's target count and selected membership roster,
independent of the history size.
Only returned rows resolve authenticated current or historical identity bindings;
a missing binding stays `None`. Receipt and removal are independent facts: a
received target may also have been removed. They represent locally retained
proof, not human reads, a complete event log, or current network activity. Pages
are successive observations, so a receipt can change between page reads.

Fixed-membership fixtures and the headless regression peer are documented
in [development tools](development.md). Normal app enrollment uses authenticated
creation, invitations and joining; it requires no fixture transfer.

## Remaining limits

Authenticated additions, owner removal, self-departure requests, invitation management,
closure and local deletion are supported by the native enrollment path. Blind
custodians and mobile key protection remain separate work. Storage exhaustion
backpressures new work; only expired content is reclaimed. The service reserves logical envelope
capacity for recipient receipts when accepting a record. An authenticated expired
record leaves a tombstone so dependent receipts can be consumed without stalling
synchronization; these bounds do not reserve filesystem or physical flash space.
Automatic polling does not imply background execution under mobile OS suspension.

Host tests cover simultaneous automatic exchange between independent Reticulum
nodes, transient database-lane saturation, offline catch-up through a member while
the author is unavailable, receipt convergence and persistent reopen. Separate
checks drop an admitted caller, exit a process immediately after acceptance, and
reopen a received record before its pending receipt was emitted. These establish
native recovery and scheduling over the test carrier, not phone/radio
qualification, Python-router interoperability or physical power-loss durability.
The prompt-delivery regression parks reciprocal polling behind a 300-second
floor, drops an admitted send's result waiter, and requires the message and
validated receipt within 12 seconds without any new receiver request reaching
the author. It then reopens both stores and delivers another already committed
record with the same ID. Scheduler tests cover failure-backoff preservation,
rotation, mandatory repair after four pushes, temporary capability expiry and
active Link progress during blocked storage queries.

## Announcement control

`run_multi_with_announcements` accepts the receiver from `announce_control`.
The controller updates a bounded periodic interval at runtime (`None` disables
startup/periodic announces) and admits manual requests through a one-slot queue.
Manual results distinguish local `Requested`, `Throttled`, `Busy`, and
`Unavailable`; none establishes peer receipt. Startup, periodic, and manual
batches share a 60-second cooldown, and configuration changes emit no traffic.
The existing `run_multi`/`run_multi_with_policy` APIs retain automatic announces
and the six-hour generic default. See the [announcement policy](transport/reticulum/docs/announce-strategy.md)
for the generic mechanism and the separate responsibilities of consumer policy
and durable scheduling.

## Bounded retry acceleration

Both native runners inspect durable outbound work before applying a failed-peer
wait. A record newer than the peer's attempted-position watermark may shorten
that wait; replaying an application request or rotating older records cannot.
After a successful repair, retained outbound work resumes at active spacing
instead of receiving an idle polling delay. Every four push attempts still yield
to an inventory/receipt repair turn, including when fresh work is waiting.

Acceleration preserves the failure exponent and ceiling. It cannot precede the
last attempt's completion plus the interface's active interval (normally 15
seconds for constrained/unknown paths, two seconds for a measured fast direct
path). `RunnerOptions.retry_initial` can raise this floor; `poll_interval` is the
idle recovery floor, so it does not park a fresh send. Repeated hints coalesce by
shortening an existing deadline once, never moving it later. Ordinary application
wakeups and announces only prompt another durable-state check.

An identified inbound request may also shorten a failed wait **when outbound
work remains**. The runner checks complete framing, reader membership, transport
identity, group, control head and store incarnation, then rechecks configuration
after the response and before applying the hint. This establishes peer
reachability; an error response does not establish content acceptance. Empty
outboxes do not produce reciprocal repair loops. The native hint queue holds at
most 32 distinct authorization scopes; overflow/cancellation falls back to normal
polling. Multi-group contact state remains capped at 256 entries, with one active
exchange per remote identity and two outbound exchanges overall. Control-head
changes preserve attempted-position watermarks and contact spacing; a different
store incarnation invalidates cursor positions without erasing the contact floor.
These are disposable hints, never ownership of ciphertext, records or receipts.

The generic `PeerSchedule` acceleration rule remains `no_std` and allocation-free.
Durable writes, request IDs, authenticated receipts, retention and crash recovery
are unchanged. Host timing tests establish scheduling behavior, not LoRa latency
or a promise that an unavailable peer will receive a message within a fixed time.

## Opt-in transfer diagnostics

For a controlled foreground investigation, set `PRNS_GROUP_DIAGNOSTICS=1` in the
process environment before startup. The native runner then emits metadata to the
`prns.groups.diagnostics` log target: static operation/error categories and frame
lengths. It never formats payloads, keys, identities, group IDs or arbitrary error
strings. A zero length means no frame length was available at that stage.
Enrollment admission, storage handling and response queueing are separate events;
queueing a response does not confirm delivery. Request Resource and response
Resource failure categories remain distinct.

Native embeddings can instead call `enable_metadata_diagnostics()` before
starting the runner and install a logger accepting the exact group targets.
Platform startup flags, property handling and stderr/logcat sinks belong to the
embedding; this crate does not install them. Producer budgets remain independent
of any sink budgets, and restarting the node does not reset a process's budgets.
A logger can filter events, so a heartbeat/readiness check is required before
claiming complete observations. The embedding owns that producer and must qualify continuous coverage
separately from bounded event capture.

The separate `prns.groups.delivery` target carries observed local send results,
push attempts and durable record/receipt transitions. Send results have a
128-event budget; pushes and receipt transitions have independent 256-event
budgets, so idle discovery and inventory setup cannot exhaust them. Their group,
record and peer correlation uses process-salted opaque tags matching the existing
local-send diagnostic; no original identifier, ciphertext or application content
is logged. An embedding must budget this sink separately from its general
diagnostic cap and retain explicit opt-in; this library does not reserve sink
lines on the consumer's behalf.

The independent `prns.groups.schedule` target emits completed attempts,
shortened deadlines, admissions and recovery-hint dispositions, capped at 1024
events in total. The `stage=schedule` categories are
`completed`, `fresh_work`, `pending_work`, `peer_recovered` and `admitted`.
`due_in_ms` and `floor_in_ms` are remaining monotonic durations; `failures` retains
the exponent history. Candidate decisions include `pending`, `fresh` and `repair`;
completion has `pending=unknown` and no new candidate decision (`fresh=false`,
`repair=false`). These events correlate a group/peer scheduling scope, not an
exact application message or physical radio transmission.

`stage=recovery` records use only `category`, `sequence`, `group` and `peer`.
They distinguish service revalidation rejection, queue offer/coalescing/fullness,
stale or unavailable scope, missing candidate, an already active peer, no failed
wait, granted credit, and the credit's eventual consumption. Consumption can
report no pending work, push probe suppression, fresh work taking precedence,
actual acceleration, or no shortening. A raw offered or credited hint is not an
accelerated deadline, admitted exchange, accepted message or receipt. Hints are
still scoped to the exact group, control, store incarnation, endpoint and
authenticated identity; an exchange in another group does not change the target
group's wait. Local interface restoration alone does not grant recovery credit.
For the multi-group runner, `peer_active` means the exact-scope evidence is
held behind that physical peer's occupied slot; it does not accelerate or admit
work while the slot is occupied. A subsequent control/store change clears the
held credit. The standalone runner still discards a hint for its already-active
attempt, whose completion determines the next schedule.

Sequence assignment and emission are serialized. The nonlogging
`metadata_schedule_snapshot()` returns the last emitted sequence and producer
health, which becomes false on exhaustion, entropy failure or poisoned ordering.
A consumer implementing continuous trial observation must expose this
watermark/readiness in its independent heartbeat and provide a separately
budgeted schedule sink. The documented observer contract allows 1026 sink lines
for the startup probe, 1024 events and the producer cap marker. Those are
consumer obligations, not a heartbeat or sink supplied by this crate. A complete
scheduling window requires the capture to match surrounding heartbeat
watermarks, including both record stages, with no relevant producer or sink cap,
missing sequence, logger failure or heartbeat gap. Missing producer support is
unqualified coverage, not a successful gate or an established network failure. Delivery evidence
has separate budgets and must still use exact durable record/receipt mappings.


## Opt-in workload measurements

The ignored workload tests are explicit developer tools. Ordinary PR, release and
scheduled runs do not execute them automatically. Smaller cases measure real
public enrollment and bounded idle queries without requiring a full-capacity
stress run:

```console
python3 tools/prns run build.groups.scale.members8 -- --output validation-artifacts/group-scale/members8-run
python3 tools/prns run build.groups.scale.members16 -- --output validation-artifacts/group-scale/members16-run
python3 tools/prns run build.groups.scale.members32 -- --output validation-artifacts/group-scale/members32-run
python3 tools/prns run build.groups.scale.members16-strict -- --output validation-artifacts/group-scale/members16-strict-run
```

These performance cases end after real public enrollment and bounded idle
queries, then close their owners. They do not exercise network delivery or
management lifecycle. The per-group profile supports 256 members; actual
membership is 8, 16 or 32. Cached cases explicitly retain one validated namespace per owner, enough
for this one-group workload. The strict control leaves that optional cache off.
This comparison characterizes the standalone owner's selected construction
policy: the product app already selects a validation cache, so a standalone cache
improvement does not establish an app improvement.

Results separate all-owner startup, each incremental join, explicit maintenance,
work-page reads, frame handling/completion, convergence checks and idle public
queries. Idle queries read active Sync work without delivering it. Public-call
counts and elapsed times are not internal transaction, disk-flush or decoded-row
counts; owning storage regressions qualify those separately. Two-way text and
receipts use an explicit separate journey, so a network wait cannot block these
admission/query measurements:

```console
python3 tools/prns run build.groups.scale.network8 -- --output validation-artifacts/group-scale/network8-run
```

Each network stage has a 30-second limit and reports early runner failure or
panic. Stage markers distinguish Publish, destination record arrival and receipt.
A failed network journey remains a failure; a completed query-only case makes no
network-delivery claim. `build.groups.scale.management8` separately retains the
Remove/Leave/Close/reopen journey. Authenticated Close supersedes an earlier
Removed state, so final convergence includes previously removed peers. Link
reservation begins only after path/key validation, immediately before
establishment; admitted and late-success cleanup remain host-owned. Neither
functional journey is required for the smaller query measurements.

The larger optional tasks remain available for later investigation:

```console
python3 tools/prns run build.groups.scale.catalog -- --output validation-artifacts/group-scale/catalog-run
python3 tools/prns run build.groups.scale.members -- --output validation-artifacts/group-scale/members-run
```

Run from the repository root, serially with other native builds and benchmarks
paused. Use a fresh output directory every time. Optional `--toolchain 1.96.0`
selects an installed Rust toolchain explicitly; otherwise Cargo uses the repository
pin. `--offline` requires the locked dependencies to be cached. The runner keeps
an existing `CARGO_TARGET_DIR`, builds the exact all-feature test target, and
locates its executable from Cargo's emitted artifact metadata. It selects test
optimization level 2 with debug assertions and overflow checks enabled, debug info 0,
no stripping and no incremental compilation. The receipt records the actual Rust
version, profile, selected artifact features, source hashes and executable hash.

Representative cases default to a 300-second measured-runtime limit (compilation
is separate). `--max-runtime-seconds` explicitly overrides it; larger optional
cases have no default time limit. A runtime halt preserves partial logs and cleans
only that run's synthetic databases. A halted run reports partial work; it must
not be described as completed membership or lifecycle qualification.

The runner supports the implemented Unix private-file owner on macOS and Linux.
It uses `/usr/bin/time` with the C locale for the isolated test process's peak RSS,
excluding compiler memory. Both build and run stop at a 5 GiB free-space floor.
Runtime databases live only in a private temporary directory under the selected
output path; they are removed after success, failure or a handled interruption.
Logs and compact JSON receipts remain. No user installation, phone, radio or
board is opened.

The catalog case creates 1,000 real one-member MLS conversations, follows bounded
live summary cursors during changes, reads exact off-page detail/rosters, and
checks Close/Delete and reopen. The optional large membership case enrolls 256
independent real MLS memberships. All membership cases deliver canonical work,
including persistent Sync probes. Completion requires actual public membership,
control and acknowledgement convergence, not an empty work queue. Four selected
members use authenticated Reticulum Links only in the selected network journey;
membership count is not concurrent network-host count. The representative
performance cases emit null network, lifecycle and reopen results explicitly.

Counts are qualification targets, not library count quotas. Selected storage,
page, transaction and transport budgets are emitted by the tests. Host timings
and RSS characterize that exact test profile and selected budgets; they do not
establish release/mobile suitability, RF behavior or physical-device acceptance.
An interrupted run records only its reached milestones. Partial membership
progress does not qualify the requested count or any messaging, lifecycle and
reopen checks that did not complete.

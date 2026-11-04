# Identity catalog and vault

`prns-identities` implements the bounded core's `Storage` contract on a
private local directory and publishes standard Reticulum identity key material.
It is a `std` adapter; the generic lifecycle core and runtime remain `no_std` and
allocation-free. It does not migrate or touch an existing application's data.

`announces::FileAnnounceScheduler<N>` separately supplies an exclusive filesystem owner
for the portable [shared scheduler](runtime/README.md#shared-announce-scheduling).
Create one dedicated directory per node, shared across its identities and
services. `create_new` and `open` are explicit, with no corrupt-state fallback.
The versioned, capacity-bound snapshot uses private permissions, an OS lock,
checksums, file sync, atomic rename and directory sync. Attempt reservation is
durable before the adapter returns a dispatch token. Ordinary timer ticks do
not write; settings, reservations and completion outcomes do. The adapter bounds
slots to 256; the caller must budget its typed snapshot and runtime memory.

`announces::Client` (`IdentityAnnounceClient` in generated bindings) submits
identity-bound controls to an embedding's existing owner lane. `settings()` and
`set_settings()` read and save `Settings`; `interval_millis: None` selects
manual-only announcements. Saving a preference does not immediately announce or
bypass the shared cooldown. `announce()` returns Queued, Throttled or Disabled.
Throttled requests remain coalesced for later admission; neither Queued nor
Throttled proves transport dispatch or peer receipt. The embedding implements `announces::Submit`, supplying one shared scheduler
and actual transport readiness.

Host tests kill a subprocess at each reservation publication boundary, then
verify old-or-new complete state, a fresh restart cooldown and no replay of the
uncertain attempt. Other tests retain partial outcomes and reject duplicate
owners, missing/corrupt data and mismatched capacity. This metadata is not message
custody, and a queued manual request is not durable delivery intent.

## Standalone key ownership

`keys::KeyOwner` owns one Reticulum identity without constructing a named-identity
catalog, coordinator, host or runtime. `create_new(path)` generates and durably
publishes a key in a new dedicated directory. `open_existing(path, expected_hash)`
opens only the existing published key and validates the caller's immutable public
identity binding. It never generates, repairs, or promotes a leftover stage.
Use separate directories for standalone keys and catalog-backed stores.

If initial creation loses its reply before the public identity is recorded,
`recover_creation(path)` returns that existing identity, completing publication
of a verified complete stage if necessary and confirming file/directory durability.
It never generates absent material, replaces a corrupt key or chooses between
contradictory records. Record the returned public identity before constructing
dependent domain state; thereafter use only the expected-hash `open_existing`.
This explicit first-creation recovery must not bypass an established identity
binding. It requires no private key inspection by the calling application.

Native provisioning can use `create_imported(path, secret, expected_hash)` and
`recover_import(path, &secret, expected_hash)` for an exact caller-supplied raw
64-byte private identity. Recovery verifies the same private material as well as
the public hash and only completes an existing matching publication. It never
generates missing material or substitutes another key. Keep private bytes in
native construction code; a foreign UI can pass an opaque reviewed capability.

`public_identity()` and `identity_hash()` expose public core identity values.
A domain retains a cloneable `KeyLease`; dropping the original owner does not
release its key or directory lock while any lease remains. The last drop zeroizes
in-memory private material and releases the lock. There are no background tasks
or asynchronous close operations in this key-only owner. A domain with admitted
work must retain its lease until that work actually finishes.

Rust construction adapters borrow the existing core `PrivateIdentityMaterial`
through `KeyLease::material()`. Foreign bindings must keep leases opaque and pass
them to native domain constructors; they must not export that accessor or secret
bytes. This module requires no catalog **instance**; catalog code remains in the
same identity library at present.

The owner reuses the catalog vault's checked key record and immutable publication
implementation, including private permissions, OS entropy, file/directory sync,
and an advisory exclusive lock. Errors leave files intact. An existing directory
is never reinitialized, including after an interrupted creation. A missing or
partial initial publication fails closed and requires explicit operator recovery;
it is not an invitation to silently generate a different identity. The same plaintext-key,
Unix-filesystem and cooperative-ownership limits below apply.

## Ownership and persistence

Use `IdentityStore::<I, O, S>::create_new(path)` once for a **new dedicated directory**.
An existing directory is an error. Use `open(path)` for every subsequent start;
missing, corrupt, incompatible, or incomplete state fails without initialization
or rollback. Paths are resolved to an absolute location before retaining them.
An interrupted initial creation with no valid catalog requires explicit operator
recovery; calling `create_new` again never destroys that directory.

Keep the store alive for the entire coordinator **and runtime owner** lifetime.
Its OS advisory lock excludes another cooperative process/store using the same
root. Do not unlink/replace the lock file or let another adapter access these
files. A second in-memory coordinator advances the durable host session; key and
runtime effects must validate against that latest durable owner. This is not a
defence against a malicious process running as the same OS user.

Normal subprocess fork/exec is supported: dropping an owner explicitly unlocks
before closing, so a briefly inherited descriptor cannot prolong ownership.
Raw fork without exec is unsupported; a child must not reuse or drop a copied
store whose file description is still shared with the parent's owner.

Use `store.runtime_authority(&coordinator)` at the reusable runtime boundary so
effect, action, admission, and quiescence checks include the latest durable
catalog owner. Asynchronous drivers must repeat authorization at execution and
report completion only after their scoped work has actually finished.

Catalog updates use a versioned capacity-specific codec, length bounds, a SHA-256
checksum, a synced temporary file, atomic rename, and parent-directory sync. An
old valid catalog remains authoritative until rename. After an uncertain storage
error the core is poisoned and must reload before further work. A leftover
catalog stage may be superseded only after reading the valid current catalog;
corrupt current state never falls back to a potentially older stage.

The encoded native envelope is limited to 4 MiB. The combined in-memory
`Catalog<I, O, S>` and bounded native creation records, and individual portable
identity/operation record types, are limited to 64 KiB. These limits are
checked before capacity-dependent stack construction, including when zero
record capacity would otherwise hide a large service-bound record type.
Native temporary snapshots use heap storage but may first be constructed on the
thread stack; applications must also budget their two caller-owned core buffers.
The host tests exercise the 4-identity/16-operation/2-service configuration on a
512 KiB worker stack. Larger dimensions need separate stack/memory qualification
or a different adapter with caller-controlled scratch allocation.

## Caller-owned creation requests

`IdentityStore::create_or_recover(&mut coordinator, request_id, intent, import)`
accepts the portable `CreateRequest<S>` unchanged. It creates a paused identity
with exactly the requested metadata and services; it does not activate a host,
select a UI identity, or insert messaging services. Native imports may pass the
existing Rust key capability. Foreign bindings must keep that capability opaque.

Read `creation_scope()`, choose a nonce, and **persist the resulting
`CreateRequestId` before submitting**. Its catalog incarnation and epoch identify
the admission scope. Every retry carries that exact ID and complete canonical
intent. A retained same-ID/different-intent request returns typed `Conflict`.
After a lost reply or restart, retrying the original ID returns the original
operation/result even after the portable operation journal has been pruned.
A new scope or nonce is new work, never an automatic uncertain retry.

The native `PNICAT02` envelope holds the portable catalog and at most `O` creation
records. Request admission and core token reservation commit together. Every
subsequent catalog CAS preserves that mapping and captures the canonical result;
result evidence and the core completed-operation floor use the **same file
commit**. Key publication remains a separate ordered operation using the existing
vault. There is no separate receipt file or app reconciler. Storage errors from a
catalog write poison the coordinator until reload; a failed key write retains the
pending core operation for exact vault recovery.

Automatic pruning advances only through a contiguous prefix of completed
operations whose exact results are retained in native creation evidence. An
independent Rust caller's core create/transition result is a barrier, even when
completed. Its owner must confirm and explicitly prune it. Native creation cannot
erase another caller's recovery token.

Call `retire_creations(&mut coordinator, confirmed_ids)` only after confirming
receipt of those completed results. Pending or unaccounted work cannot be removed.
If another caller's journal entry prevents safe pruning, retirement returns typed
`PruningBlocked` and retains the evidence. Otherwise removal and epoch advancement
commit atomically. Retrying a lost retirement acknowledgment is safe. Pending and
unconfirmed requests from earlier epochs remain recoverable; absent old IDs return
`Expired` and can never create a new identity. Future epochs and other catalog
incarnations are rejected. Filling the bounded evidence table returns `Capacity`;
there is no implicit eviction, rotation, timestamp policy, or regeneration.

Caller evidence uses native envelope `PNICAT02`; explicit service configuration
uses portable catalog/codec version 2 inside it. The key format is unchanged.
Opening `PNICAT01` returns `UnsupportedCatalogFormat`; an older portable codec
returns its typed unsupported-version error. Neither is rewritten or replaced
with an empty catalog. The native
snapshot bound includes both the portable catalog and bounded creation records.
Tests interrupt request admission, key publication, result recording, pruning and
confirmation at file-publication boundaries, and exercise independent callers,
old pending IDs, changed intents, capacity exhaustion and catalog replacement.

## Keys and retries

1. Reserve a creation operation and retain its token; begin that same operation.
2. Pass its current `PublishKey` ticket to `publish_key`. Generation uses OS
   entropy through the existing PRNS entropy abstraction. Import checks the
   public identity hash with the existing Reticulum key implementation.
3. A fixed, checksummed key record binds the operation-derived reference to its
   public hash and 64-byte Reticulum private key. The stage is synced before
   rename and directory sync. Only verified durable readback returns a
   `VerifiedKey`; give that witness to the core's `ack_key`.
4. If a reply is lost, reopen the store/coordinator and obtain the fresh ticket
   for the **same operation**. A complete stage or final key reopens the same key;
   incomplete/corrupt material fails closed. It is never replaced with fresh
   entropy. A crash before any key stage exists can safely retry generation,
   since no key was published or acknowledged.

`activate_key` accepts only a current activation/restoration action and reads the
existing final key. It never repairs a missing key, generates new material, or
promotes a stage while restoring an already-published identity. Scoped service
drivers restore their own durable receive state before opening ingress; reading
an identity key alone does not establish service readiness.

A competing import can leave a second retained reference to the same identity.
Call `account_retained_key`, then `acknowledge_retained_key` on the core, before
pruning that operation result. The durable retention index survives result
pruning. There is no automatic key deletion or secret-retirement claim here.

## Security and platform limits

The optional `redb-store` feature exposes `scoped_store::ScopedDatabase` for
native service storage. Each file has an immutable binding to the catalog
incarnation, local identity sequence and public identity hash. Clients retain
that owner for their lifetime; a selected identity is never resolved at execution
time. Local record IDs, request IDs and cursors must always travel with their
storage scope. Different local identities may legitimately have the same local
record ID or retry token.

`StorageScope::path_under` provides a deterministic filename without labels or
public-key-dependent paths. Creation and reopening are explicit operations;
missing or different bindings fail without initialization. The caller initializes
and validates its service schemas separately, so a valid scope marker alone is
not service readiness. This isolates existing contact, direct-message and group
table formats without teaching them UI identity selection. Lifecycle admission,
draining accepted jobs and protecting secret contents remain caller obligations.

This native-only optional dependency never enters portable core, codec or runtime
builds. It does not open multiple writers for the same file or move installation
management/Bluetooth preferences into a persona database.

This initial adapter stores **plaintext private keys** in directories with Unix
mode `0700` and files with mode `0600`. Permission checks reject symlinks,
nonregular files, hard-linked files, and group/other access. This is not encrypted
storage, hardware-backed key protection, rollback resistance, or protection
against a same-user attacker who can replace both data and checksums. Protected
phone storage remains deferred.

The durability contract requires a Unix local filesystem supporting atomic
same-directory rename, file sync, directory sync, and OS advisory locks. The
current evidence is host process-crash recovery on macOS; real power-loss,
mobile-filesystem, phone permission/protection, and device qualification remain
separate work. Unsupported platforms or failed syncs return errors.

Run host checks with:

```sh
cargo test --manifest-path prns-identities/Cargo.toml -p prns-identities
cargo test --manifest-path prns-identities/Cargo.toml -p prns-identities --features fault-injection,redb-store
```

The optional `fault-injection` feature enables test subprocess exits at catalog,
key, and retention-index write boundaries. It must not be enabled in a shipping
build; normal builds do not inspect the crash-control environment variable.

## Service configuration integration

Use the portable retained AddService/RemoveService transitions and the runtime's
explicit synchronous preparation helper. `initialization_key` grants access to the
existing key only for the current `InitializeService` effect; ordinary history or
activation authority cannot initialize a missing service schema. Keep preparation
under the same exclusive native dispatch gate. No host, messaging service or app
selection is required.

The filesystem fixture uses an independent telemetry domain: its durable
preparation survives reply loss and native catalog reopen, an omitted adapter
keeps the operation pending, unsupported state is preserved, and original native
creation evidence still resolves after service addition/removal. These fixtures
do not implement asynchronous initializer ownership or a concrete group adapter.

### Owning catalog and runtime lane

Enable the optional `owner` feature for this lane. The default/key-only graph does
not include Tokio or this executor adapter.

`owned::Owner<I, O, S, L, D>` keeps the exclusive `IdentityStore`, both coordinator
buffers, runtime ownership table, native driver, and key leases on one bounded
worker. For example, `Owner<4, 32, 4, 16, IdentityOnly>` supports four identities,
32 retained operations, four configured service slots per identity, and 16
runtime mutation slots. `Options` requires an explicit queue capacity and runtime
resource budget; native memory bounds are checked before construction. There is
no host, Tokio runtime, messaging service, or selected-identity state. Tokio's
sync-only channel supplies result futures that work under any executor.

Construct with `create_new` or strict `open_existing` off the UI thread. The driver
factory receives the worker's weak/coalesced progress waker before readiness. It
may assemble state, but cannot acquire live identity resources until a scoped
step runs. Open validates the catalog and starts a new durable session; it does
not restore enabled identities automatically. `IdentityClient::recover` explicitly
restores the current action of its immutable core `IdentityReference`. Each queued
command checks that binding and current exclusive catalog authority.

The non-generic `api::Client` (`IdentityCatalogClient`) and `IdentityClient` are
one public contract for Rust, generated bindings, and compositions. Their async
methods admit bounded work on first poll; a future dropped before polling submits
nothing. Once admitted, dropping the result future does not cancel the work.
Canonical native aggregate records use checked Vec projections of the fixed-bound
core. Conversion preserves sparse dependency-report slots for exact command
replay and rejects excess capacity without truncation. Core/scalar models are
reused directly. The optional `uniffi-bindings` feature derives metadata on these
owning declarations; default portable graphs do not acquire UniFFI or `std`. Creation retains the native
caller-owned request ID. Lifecycle calls reuse core prepare/reserve/begin and
operation lookup: persist the inert reserved token and complete prepared command
before `begin`. Exact result lookup never chooses a new request automatically.
After an ambiguous catalog write, drain/close and reopen the owner before
retrying the same retained command; the owner does not discard resources to
reload a poisoned coordinator around a still-live driver.

`owned::Driver` adds only construction, dependency inspection, key binding, and
typed failure classification to the existing `ScopedDriver`/`ServiceInitializer`
protocols. It owns every asynchronous future and captured scope until settlement.
A `Pending` result wakes and resumes this same worker; fresh dispatch happens only
inside a freshly authorized scoped call. One pending identity does not block
another identity's lane work. The synchronous service initializer remains limited
to durable preparation without live resources; resource-owning initialization is
not introduced by this adapter. Unknown configured services fail explicitly.
A creation request with a nonempty initial service set assumes those schemas are
explicitly provisioned by its embedding; creation and ordinary activation never
initialize them implicitly. The self-contained workflow creates an empty service
set, then uses retained `AddService` transitions for durable preparation.

A driver's `bind_dispatch` receives a read-only `DispatchLease` bound to the exact
runtime owner before preparation. It starts closed, opens only after canonical
activation confirmation, and closes before a fresh lifecycle transition writes
its catalog intent. `dispatch_opened` receives the existing canonical
`NetworkPermit` once per selected service after confirmation; it only installs
that exact permit and wakes an already-owned runner. It cannot acquire resources.
Identity-only activation sends no network notification. Old completed transition retries do not fence a replacement
owner. Once fenced, a failed catalog publication cannot reopen that generation.
The physical worker's close signal also closes every lease synchronously on close
or owner drop, independently of how soon its lane can run cleanup. The lease owns
no key or worker and grants no authority; network work still needs exact canonical
admission and a retained key capability. Cleanup and completion routing remain
installed until actual drainage.

Native `owned::KeyLease` borrows the existing core key material; no secret export
API is added. The worker retains its store lock through all actual key leases,
including a lease retained by a pending effect. Returning a release receipt while
holding a lease cannot release the catalog owner early. Driver implementations
must keep their own future/resource allocations bounded by their declared plan.

`close()` fences admission immediately and returns the worker's retained,
cloneable completion. Queued requests whose handlers have not started receive
`ApiError::Closing`; already-started effects retain ownership through cleanup. Close
uses ephemeral runtime termination and never persists Pause or acknowledges an
unfinished lifecycle result. Repeated or dropped close observers and owner
finalization do not abandon pending work. `diagnostics()` returns a cloneable observer retained independently after
admission closes or the owner drops, so an unavailable adapter that cannot prove drainage is visible;
its incomplete cleanup keeps completion pending rather than claiming success.
The same bounded observer retains the latest rich synchronous handler or lifecycle
error. A diagnostic has an optional identity: catalog-wide and pre-creation
failures have none. Each newer error replaces the prior observation; this is not
an error history. Ordinary clients return the canonical typed `ApiError`, and
snapshots project the scoped lifecycle failure to the same classification.
Neither classification nor I/O kind establishes whether a write had no effect.

A driver panic is a failed completion, not proof that all external work drained.
Each native key capability also holds the catalog's exclusive lock independently
of the worker stack: an unwinding owner cannot release that lock while an external
effect retains its key lease. The last lease releases the lock with the same
explicit-unlock semantics used by the key-only owner.

### Composed domain commands on the same lane

`owned::run` is the borrowed form of the same engine used by standalone `Owner`.
An embedding creates its physical databases and borrowing managers lexically in
the worker initializer, then supplies a driver factory that may borrow them.
There is no `'static` driver requirement on `run`, no self-referential manager,
and no extra worker per domain. The embedding constructs identity clients with
`Client::from_lane(worker.client(), worker.control_client(), O, S)` using the same
operation and service bounds as `run`. Its outer initializer keeps all stores alive and drops
them before worker completion.

The queue has an optional generic typed payload `J` (default `Infallible`). A
`CommandHandler<S, J>` receives that domain's existing job plus a borrowed
`Admission` context. `owner.commands()` or construction-time `Commands::from_lane` supplies only ordinary bounded queue
submission; it cannot set access classes or use the lifecycle control slot.
Bindings can therefore return the ordinary domain client through its native
Submit adapter. The identity crate contains no domain method enum, domain
registry, or callback/closure executor. A composition with additional domains can
route their job types with a typed sum; it need not mirror their operations.

Only a trusted handler derives `Access` from the actual command and durable
receipt evidence. Pure reads use History; exact admitted work uses Recovery.
Fresh private edits use LocalMutation, and eventual service work uses
ServiceMutation. The context validates the current locked catalog and grants an
opaque owned `Permit`; service mutations also retain the existing runtime lease
and native key lifetime. Exact durable lookup precedes fresh mutation admission.
The handler does its bounded storage work synchronously under this one gate,
then explicitly settles the permit. Dropping a permit never drains a mutation.
`Settlement::RecoveryRetained` means a confirmed durable record now owns an
unresolved attempt; it does not assert a final result or an uncertain commit.
No transaction, borrowed authority, or pending command future crosses an await.

An uncertain physical effect keeps its original `Permit` in the owning domain's
bounded state. `Driver::poll_recovery` runs before lifecycle progress, including
while closing, with only `RecoveryAdmission`: validate that exact identity,
service, operation and permit under current Recovery authority, then settle it
from actual domain evidence. `settle_existing` takes the permit out of its slot
only after successful canonical settlement. This hook cannot acquire new work or
network authority; its weak waker can finish an accepted effect after all ordinary
clients close. A failed or unavailable recovery remains owned and delays drainage.

Pause, Retire, and RemoveService prepare/reserve/begin use the physical worker's
single reserved control slot. Ordinary queue saturation cannot consume it. That
capability is retained inside the identity client and is never exposed by the
domain submitter. Accepted ordinary jobs execute after the stopping transition
under current authority; fresh work is then rejected, while exact recovery stays
possible. Local domain storage is a separate lexical lifetime from activation
key leases, so paused history/private edits do not keep network key use alive.

For Pause/Retire at ordinary journal saturation, use `IdentityClient::apply_stop`
with the caller's persisted prepared command and digest. It invokes the existing
core atomic stop operation and protected stop capacity. Repeating that exact
pair recovers its retained token after reply loss. The general inert
reserve/persist-token/begin path remains available for other transitions; it
cannot consume a protected stop slot merely to reserve fresh work.

RemoveService uses reserved queue admission for its stopping preparation, but
retains ordinary configuration journal semantics. Only core Pause/Retire
`apply_stop` consumes protected durable stop capacity; the queue reservation does
not promise journal space for arbitrary reconfiguration.

Lost inert reservation replies are recoverable through canonical `find_reserved`
and bounded `operations` inspection. `cancel_reserved` cancels only an unstarted
reservation. After callers have retained their completed outcomes,
`prune_completed` explicitly advances the core floor; pending work or unaccounted
key material cannot be pruned. Expired tokens stay rejected after reopening.

## Optional offline contact cards

`contact-card` adds service-neutral signed claims using the existing
`prns-contact-card` codec. This feature alone enables neither the native owner nor Tokio and requires no
groups, LXMF, host or named-identity catalog. A standalone
`keys::KeyLease::contact_card(service_name_hash, display_name)` synchronously
returns canonical signed public bytes while retaining its existing key owner.
The allocation-free codec and its `ContactCardSigner`/core-identity adapter remain
independently usable.

With `owner`, the canonical `IdentityClient::contact_card` admits the same bounded
operation on its existing worker. The codec validates names before queue admission;
the foreign binding checks the exact ten-byte service hash. At execution the
catalog obtains fresh `OfflineSigning` authority and loads only the pinned existing
key through `IdentityStore::local_key`. Paused identities may issue cards without
activating services; pending or retired identities cannot. Wrong identities and
missing/corrupt keys fail without generation, schema initialization or catalog
mutation. No private key or unrestricted signer escapes the job.

Cards assert a service/name claim. They do not prove service availability,
reachability, peer consent or membership. The consumer selects and verifies the
supported service, chooses its own contact/trust policy and transports the bytes
or the codec's canonical URI. Repeating an identical claim signs the same bytes;
there is no second operation journal or retained network effect for offline signing.

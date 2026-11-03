# Identity ownership and lifecycle

`prns-identities` owns the filesystem catalog, key vault and identity work lane.
Start with the [usage guide](usage.md) for ordinary Rust and foreign clients.
The lower crates below keep lifecycle rules and runtime admission portable.

`prns-identities-core` coordinates independent local identities without depending
on messaging, Reticulum routing, a database, an executor or a platform key store.
Its production code is `no_std`, allocation-free and forbids unsafe code. The
caller supplies storage, two bounded catalog buffers and service/key adapters.
Applications choose labels, selection and service policy independently of this
contract. The [identity owner](usage.md) and its generated clients expose
the same lifecycle operations to Rust and foreign consumers.

| Crate | Responsibility | Environment |
| --- | --- | --- |
| `prns-identities-core` | Durable lifecycle, admission and exact-operation recovery | `no_std`, allocation-free, caller-owned buffers/storage |
| [codec](codec/README.md) | Bounded, versioned catalog encoding and validation | `no_std`, allocation-free |
| [runtime](runtime/README.md) | Scoped ownership, resource admission, mutation drain and shutdown evidence | `no_std`, allocation-free, caller-owned tables |
| [identity library](usage.md) | Atomic filesystem catalog, Reticulum key vault, durable runtime authority and optional scoped service databases | `std`, Unix local filesystems |

Service adapters can connect selected services to this owner's lane and
immutable identity admission. Independent
identity-only and non-messaging consumers use the identity owner directly. Domain
clients keep their original identity binding across application selection changes;
the embedding does not add a second domain method facade. Embedded flash/runtime
adapters remain separate work.

## Storage and effects

`Catalog<I, O, S>` bounds saved identities, retained operations and services per
identity. Identity records retain opaque key references and public hashes, never
private keys. Local labels and published names have separate bounded fields.
Metadata changes do not change lifecycle revisions or existing send ownership.

`Storage` loads into caller memory and atomically compares/stores a complete
catalog revision. A successful write must be durable. A failed write may have
committed: the coordinator becomes unusable until it reloads from storage.
The storage adapter owns checksums, encryption, atomic replacement, flash wear
and locking. The core does not detect replay of an otherwise valid old catalog;
rollback protection, if needed, belongs to that adapter. It must serialize
lifecycle ownership with service admission; a
compare-and-store method alone is not a distributed runtime lease.

Public catalog fields allow an adapter to encode its own versioned schema and
reconstruct typed values without unsafe memory conversion. Loaded catalogs are
validated before use. Rust struct memory layout is not a file format. Two caller
buffers let the coordinator stage writes without allocating or returning an
owned catalog snapshot on its stack.

Use a fresh, nonzero incarnation for each newly initialized catalog and preserve
it on reload. It scopes IDs, operation tokens and key references across catalog
replacement. The adapter must not silently initialize an empty catalog after an
unreadable load. Bounds apply to retained identities (including retired ones),
operation records and services; capacity failure never evicts them. Labels allow
64 UTF-8 bytes and published names 128, without control characters.

Creation and activation reserve a catalog-issued token before beginning. The caller retains
that token and reuses it after an uncertain result. Reserved operations remain
discoverable after a lost reservation reply; they perform no external work until
explicitly begun. Completed-prefix pruning retains a replay rejection floor.
Retirement does not free an identity's ID for reuse.

Use `apply_stop` for Pause/Retire: it atomically records intent and issues (or
reuses) a token, with no external teardown inside the call. Repeating the exact
prepared transition and digest recovers an uncertain result. Ordinary reservations
leave one free stop slot for every Enabled identity, including a reserved Enable.
Only that identity's atomic stop consumes its reserve; unrelated work cannot
strand it online when the journal fills. Unstarted transitions for that identity
are cancelled atomically. Already-started activation/cleanup must finish first.
Paused identities have no reserved retirement slot. An unfinished early operation
can still block prefix pruning and new ordinary work; recover it before admitting
more. The library does not evict pending work or recycle identity IDs to make room.

Importing an existing public identity resolves to its original record, including
a retired record. If another publication wins after an import has already
created a vault reference, that extra reference remains recorded in the result.
The caller must durably account for it before `acknowledge_retained_key` permits
pruning; no lifecycle operation authorizes deleting an uncertain/imported key.

The coordinator emits scoped effects after persisting intent:

- Publish/reopen a key under the reserved reference. The trusted vault adapter
  verifies durability and the key-derived public hash before acknowledging it.
  It must never regenerate a different key at an uncertain reference.
- Activate only the named identity's services after resource admission and
  restoration of retained receive keys.
- Quiesce those services, drain admitted mutations, preserve durable work and
  report authoritative dependencies before completing pause/retirement.

Effects are replayable and fenced by operation, identity, lifecycle revision and
host session. Adapters must make replay idempotent. A new session must first fence
any old runtime owner; losing a coordinator object does not itself close a live
Link. Frames already sent cannot be recalled. History and exact result recovery
remain separate from permission to admit fresh network work.

Check `validate_effect` under the runtime's ownership gate immediately before an
effect, and `validate_admission` under the service's admission gate before fresh
work. Checking a completion callback alone cannot prevent an old worker from
changing external state. The host owns authentication and authorization for API
callers; IDs, digests and tickets are correlation/fencing values, not credentials.

Enabled, Paused and Retired are durable local states. Enabling does not establish
reachability or background execution. Retirement requires a complete dependency
report after quiescence; changed or unresolved obligations leave the identity
paused. Retirement neither revokes group membership nor destroys keys.
Dependency revisions track durable obligations, not transient runtime status;
the act of quiescing alone must not invent a changed dependency revision.

| Admission | Allowed lifecycle state | Additional contract |
| --- | --- | --- |
| History / Recovery | Any | Recovery only looks up or settles already admitted work under its original owner; no fresh execution |
| LocalMutation / OfflineSigning | Stable Enabled or Paused | Local contacts, drafts and settings; explicit contact-card signing |
| ServiceMutation | Stable Enabled | New durable publication/enrollment may be accepted offline; the service checks its store readiness |
| Network | Stable Enabled | Runtime activation/restoration must also be acknowledged |

Pending transitions and restore cleanup close fresh admission. After reopening,
Enabled identities need service restoration before network work resumes. A
failed restoration keeps a durable cleanup fence in the identity record itself,
so cleanup remains possible without allocating another operation slot.

## Explicit service configuration

Compilation, configured ownership, current activation and stored obligations are
separate facts. `TransitionTarget::AddService(id)` and `RemoveService(id)` use the
same `prepare` / `reserve` / `begin` journal as other lifecycle transitions.
Service IDs are stable domain identifiers, never positions in a feature list.
Duplicate additions, absent removals and capacity errors are rejected before
side effects. Configuration does not consume protected stop slots through
`apply_stop` and does not alter group membership or any other domain protocol.

Both operations fence fresh admission and quiesce the complete current service
set. Pass the fresh, complete dependency report to
`ack_configuration_quiesced`. Addition then retains an `InitializingService`
phase and emits `Action::InitializeService` for the one absent service. The domain
idempotently prepares supported, identity-bound durable state using the retained
operation token. Only `ack_service_initialized` adds its ID to the configured set.
A domain commit followed by reply loss or catalog failure therefore remains an
explicit pending preparation on restart. An unavailable adapter cannot make that
operation disappear or allow activation/retirement to bypass its pending state.

Removal requires authoritative evidence from the removed service, including its
actual pending messages, control, recovery and key/authority obligations. A changed
review or remaining blocker yields `ServiceRemovalBlocked`; its ownership entry
stays configured. Obligations belonging only to services that remain configured
are preserved and do not themselves block removal. Missing or incomplete adapter
reports cannot substitute an empty report. Successful removal drops the service
ID; it does not erase domain data or keys. Domain adapters must not report clear
while their retained state still requires an owned capability.

Both successful operations leave the identity **Paused**. Enable is a separate
explicit transition. Ordinary activation/reopen validates every configured
service's existing schema; it must not call initialization to repair missing
configured state. Adding an absent service is the explicit preparation boundary,
including validation of any retained domain state. Unsupported formats fail
without replacement.

`IdentityRecord::initial_services` preserves the immutable creation fact while
`services` records current configuration, so old create results still validate
after additions and removals. These states use core catalog format 2 and
codec version 2. Version 1 fails closed; there is no implicit migration or reset.
The filesystem envelope and identity key format are independently versioned.

## Managed nodes and infrastructure

Lifecycle effects address application services, never the whole shared node.
`other_key_uses` reports infrastructure ownership without importing RemoteControl
types into this library. An adapter must preserve unrelated key holders, Links,
pairing grants and interface state; it cannot use whole-node shutdown as a persona
pause operation. Capacity admission must leave management/pairing reserves intact.

Infrastructure credentials can remain separate from an application's local identity.
Forwarding another device's traffic does not require a copy of its identity or
service secrets. A device hosting its own identity needs its own durable storage
and scoped runtime adapter; see the [runtime contract](runtime/README.md).

## Runtime integration

The bounded controller reserves a complete service plan within a shared resource
budget, withholding infrastructure headroom. It requires old-worker fencing,
service-store/receive-state restoration and acknowledged activation before network
dispatch. Mutation leases account for admitted work during stopping; dropping a
lease is not cancellation. Per-service shutdown must report completed ingress,
egress, Link, announcement, destination and key-handle cleanup. Submitting a close
command is insufficient evidence.

Use `IdentityStore::runtime_authority(&coordinator)` with the runtime controller so
dispatch checks the latest durable session as well as the in-memory coordinator.
Keep caller-owned runtime tables through controller recreation. After uncertain
catalog acknowledgements, reconcile the affected owner rather than clearing the
whole table or disturbing unrelated identities. Driver scopes must be checked
again at execution for queued work.

A concrete service driver must supply live destination, Link, announcement,
operation and retained-buffer barriers. Staged registrations stay closed until
network admission; stale crypto completions cannot revive a retired destination.
Host-buffer handoff is not physical radio transmission or peer receipt. The
[identity owner](usage.md) preserves these driver obligations without
installing a particular service or host.

## Checks

Run from the repository root:

```sh
cargo test --manifest-path prns-identities/Cargo.toml --all-targets --no-default-features
cargo test --manifest-path prns-identities/Cargo.toml -p prns-identities --all-targets --features owner,fault-injection,redb-store,contact-card
cargo clippy --manifest-path prns-identities/Cargo.toml --all-targets --no-default-features -- -D warnings
cargo clippy --manifest-path prns-identities/Cargo.toml -p prns-identities --all-targets --all-features -- -D warnings
cargo build --manifest-path prns-identities/Cargo.toml --no-default-features --target thumbv7em-none-eabihf
cargo build --manifest-path prns-identities/Cargo.toml --no-default-features --target riscv32imac-unknown-none-elf
cargo run --manifest-path prns-identities/Cargo.toml --example telemetry
```

The workspace defaults to its three portable crates, so bare-metal builds never
pull in the filesystem adapter. Filesystem fault-injection tests terminate subprocesses
at filesystem publication boundaries; never enable that feature in a shipping
build. Current filesystem evidence covers macOS process crashes, not electrical
power loss, phone filesystems/UI or live RemoteControl integration. The telemetry
example uses fixed caller-owned storage and simulated keys/runtime; it is an
independent non-messaging consumer. The standalone foreign consumers
qualify key-only and catalog profiles separately; retain exact-build and device
acceptance receipts separately.

## Adapter metadata checkpoints

`Coordinator::checkpoint(storage)` commits the unchanged logical catalog with a
new revision through the normal storage CAS. The filesystem adapter can co-commit its
own bounded metadata without resetting the runtime session or readiness.
Successful checkpoints preserve existing admission authority. Any storage error
may have committed; it poisons the coordinator and clears readiness until an
authoritative reload, exactly like a lifecycle commit. The portable core does not
interpret that adapter metadata. The filesystem adapter's caller-owned creation API
and bounded result-retirement contract are described in its
[README](usage.md#caller-owned-creation-requests).

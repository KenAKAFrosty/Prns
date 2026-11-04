# Identity-scoped runtime controller

`prns-identities-runtime` is a `no_std`, allocation-free controller above
`prns-identities-core`. It reserves bounded resources, gates service work, tracks
admitted mutations, and requires explicit completion evidence before acknowledging
an identity's shutdown. It does not own keys, service databases, threads, clocks,
interfaces, or a Reticulum node.

## Shared announce scheduling

The independent `announce` module schedules application announcements for one
shared node owner. It is also `no_std` and allocation-free. Callers supply two
`Snapshot<N>` buffers, `Runtime<N>`, a durable `Storage<N>` adapter, monotonic
milliseconds, optional wall-clock observations and a random `u16` per dispatch.
The key is a catalog-local identity plus an opaque service ID; a channel bitmask
describes the destinations in that service's batch. No messaging or radio type
enters this module.

`reload` validates storage and conservatively waits 60 seconds after any prior
attempt. `configure` sets activation, channels and an optional periodic interval.
`None` means manual only, including no startup announcement. Enabled periodic
services coalesce startup work and wait for an eligible interface. The caller
must reconfigure a paused identity before dispatch and validate current lifecycle
admission at execution; scheduler reservations are not authorization credentials.

`request_manual` retains one coalesced volatile request. `Throttled` also retains
the request; neither response means it was sent. `reserve_next` rotates fairly
through manual and automatic work, with one shared batch per 60 seconds and a
60-second owner cooldown. It durably reserves an attempt **before returning a
dispatch token**. Failure or uncertain host acceptance still spends that budget.
`reserve_next_matching` lets the embedding skip temporarily busy owners without
spending their pending intent or the shared budget. Other owners remain eligible.
`complete` records disjoint accepted/unconfirmed channel masks, preserving partial
submission outcomes. These describe host acceptance, not physical transmission.

Intervals receive 0–25% positive jitter. Missed periods become one pending batch;
selection, renaming and preference edits do not reset reservations. Changing the
interval schedules the next automatic period rather than itself announcing.
Pause clears only that owner's pending fresh work. Manual work has no priority
over automatic work and cannot starve other identities. Requested Reticulum path
responses stay outside this application scheduler.

Atomic storage errors fence the owner until reload. Reboot and clock rollback
start a fresh cooldown; incomplete attempts are never replayed. Pending manual
intent is intentionally volatile: applications must say queued, not delivered,
and can request another announce after restart. No writes occur for ordinary
timer observations. Slots include paused/retired owners so their history cannot
be evicted to evade cooldowns. Storage rollback protection remains an adapter
responsibility. Tests cover fairness, restart/rollback, partial results, bounded
capacity, pause/isolation and errors on either side of durable reservation.

## Integration contract

Keep one caller-owned `RuntimeState<I, S, L>` alive for the host's lifetime and
borrow it with `Controller::new`. `I` bounds retained owners, `S` bounds services
per owner, and `L` bounds concurrent mutations across all owners. One additional
cleanup-only owner is reserved independently of `I`, so a rejected activation can
still clean up when normal owner slots or numeric resource budgets are full. That
slot grants no new activation resources: it represents unknown external resource
debt and blocks further activation until its cleanup is confirmed. Existing running
owners and reservations remain intact. There is no heap
allocation or implicit table growth. Use `core::mem::size_of` for the chosen target
and capacities; place the table explicitly in the application's storage/stack
budget. Controller recreation preserves all leases, reservations, and generations.

`Budget` reserves peak keys, destinations, handlers, Links, and queue entries for
each owner. `infrastructure_reserve` is withheld from **every** application plan;
include peak pairing/management demand, not just current consumption. The driver's
preflight also checks actual host availability and enforces those per-owner limits.
A reservation counts until confirmed scoped cleanup, including partial activation
and uncertain driver failures. The controller never releases another owner's key
handle based on a matching public hash.

An `Authority` must validate the current durable session and full lifecycle action.
The `Coordinator` implementation is sufficient only with externally guaranteed
exclusive ownership. Native consumers should use `IdentityStore::runtime_authority`
to reject stale coordinators using the same store. Serialize authority validation,
dispatch, and lifecycle changes under the embedding host's exclusive owner gate.

Driver calls carry an exact `Scope` containing owner incarnation/sequence, local
identity, lifecycle session/revision, generation, service, step, and optional
termination authority. Every queued worker must revalidate that authority and
scope **at execution**, not only enqueue, using `validate_scope` under the same
dispatch gate.
`Pending` and driver errors retain all state; retry the same idempotent scope.
Evidence for a different scope cannot advance the state machine. The incarnation
must be unique across runtime-table replacements; do not reuse/reset the table
while old workers can still act.

## Activation and service use

1. Obtain the core's current `Action::Activate` and supply its exact service plan
   to `begin_activation`. This atomically reserves the whole bounded plan.
2. Complete `fence_previous_workers`, even on an apparently fresh activation.
   The driver must fence prior generations and reconcile old resources by their
   recorded local owner; a new RAM table does not prove old workers disappeared.
3. Complete `preflight`, then `prepare_service` for every service. Preparation
   verifies/restores the existing key and service store, including retained
   receive ratchets, before registration. It must never generate a missing
   previously published identity key.
4. Complete `activate_service` for every service. Registrations stay behind the
   controller admission gate. `finish_activation` returns a receipt only when all
   are active. Acknowledge its ticket through `core.ack_activated`, then call
   `confirm_activation` with a current core `Network` admission guard.

`admit_mutation` accepts core `ServiceMutation` guards as soon as the service's
storage is prepared and the identity permits fresh work, without requiring
network readiness. That supports durable offline publication. It also accepts
`Network` guards after confirmed activation for mutations whose preparation can
emit traffic, such as recipient path resolution. It returns a bounded
lease for one service-owned request ID. The service must still persist its exact
owner/request record and deduplicate durable retries; the volatile lease table is
not an outbox or transaction log.

Only `Network` guards can produce a `network_permit`, and only after confirmed
activation. Revalidate it at the actual dispatch boundary. Dropping a mutation
lease value does not drain it. `complete_mutation` requires a definitive committed
or rejected outcome; uncertainty retains the lease. Existing admitted work may
settle after pause/session changes, but the old lease cannot be used after it has
drained. `Recovery` is not fresh mutation admission: the service independently
checks original durable record ownership before exact-result recovery.

## Service configuration preparation

Core AddService/RemoveService operations first use the same scoped quiescence
steps as Pause, including prior-worker fencing, accepted mutation drain, actual
resource release and fresh dependencies from every configured service. Acknowledge
that receipt with `ack_configuration_quiesced`, then `confirm_quiescence`.
The latter recognizes an addition's durable intermediate initialization phase;
releasing old runtime bookkeeping does not complete the pending catalog operation.

Only an explicit `Action::InitializeService` may enter
`configuration::initialize_service`. The helper validates the complete action and
checks the returned service/ticket witness. `ServiceInitializer` is synchronous:
it prepares or reconciles durable domain state under the owner gate and must leave
no live workers or resources on either success or error. A partial durable write
is recovered using the same operation and identity binding. Missing adapters and
unsupported state fail without synthesizing readiness. Async/resource-owning
initializers require separately retained ownership and are outside this interface.

After `ack_service_initialized`, the identity remains paused. Existing
`ScopedDriver::prepare_service` is the ordinary activation/restoration path: it
validates configured state and restores receive material, and never initializes
missing state. An activation plan omitting a configured service is rejected;
compiled features cannot silently change catalog ownership.

## Process owner termination

Closing a process owner must not persist Pause or change saved enablement.
`Controller::begin_termination()` is an infallible, repeatable terminal fence for
this exact `RuntimeState`. It retains every ordinary and cleanup-only owner,
reservation and mutation lease, and reuses the existing prior-worker fencing and
quiescence phases. Activation, restoration, ordinary lifecycle entrypoints,
mutation admission and network permits then return `Terminating`. There is no
reset/reopen operation on this runtime table.

The returned `TerminationToken` is bound to the runtime incarnation. Driver
`Scope::termination` explicitly separates this cleanup authority from ordinary
lifecycle authority, including when the generation counter is exhausted. It may
only fence previous workers, quiesce a configured service, or release that exact
owner's resources. It does not authorize traffic, fresh mutations, storage
initialization or catalog acknowledgments. Runtime incarnations must remain
unique across table replacements. The original lifecycle ticket in a termination
scope is provenance, not current catalog authorization.

Enumerate the retained `RuntimeState::owner_tokens`, settle accepted mutation
leases with their definitive outcomes, then drive `fence_previous_workers`,
`quiesce_service` and `finish_quiescence` as usual. Missing adapters, Pending
results, errors and incomplete reports retain ownership and prevent completion.
Even a previously quiesced owner repeats the qualified fence/report under its
termination scope. Before actual queued dispatch, call `validate_scope` under the
same exclusive gate; it checks the exact owner, generation, service, step and
current phase. Old ordinary callbacks cannot become termination effects.

Ordinary scopes still validate fresh durable `Authority`. Termination instead
uses the retained runtime capability to stop resources that this table already
owns, so a poisoned or unreadable catalog cannot itself prevent shutdown. It may
not discover or adopt unrelated resources by a shared public hash. Complete
resource/key/receive-state evidence is still required; failed storage flushes
cannot be called drained.

Pass a termination receipt only to `confirm_termination(token, receipt)`.
`confirm_quiescence` rejects it, and its historical ticket must never be passed
to a core acknowledgment. Confirming removes only that exact fully drained
runtime owner. `termination_complete(token)` stays false until all retained
owners, mutation leases and reservations are gone. Native owners must also drain
any additional pending effects they own before releasing the catalog lock or
publishing physical close completion. The native admission gate must fence every
fresh job category, including operations outside this runtime controller.

No catalog field, revision, desired state or pending durable operation changes.
After actual shutdown, a new coordinator session can restore an Enabled identity
or recover the original pending operation using the existing lifecycle protocol.
Dropping a caller, controller borrow, token or receipt never performs this drain.

## Pause, retirement, and recovery

1. Persist the core stop operation (`apply_stop`), then pass its current
   `Action::Quiesce` to `begin_quiescence`. This fences fresh local work and network
   generations without losing already admitted mutations. Finish any outstanding
   previous-worker fence when required.
2. Settle all tracked mutations before `quiesce_service`. Each service must prove
   its ingress, outgoing/incoming Links, queued egress, schedules, and handlers are
   stopped, receive-key changes durable, and key handles reconciled. Submission of
   a close/unregister command is insufficient.
3. `finish_quiescence` calls `release_identity`. The driver releases only owned
   reservations, preserves shared owners, and returns a **fresh authoritative**
   dependency report after all drain barriers. Keep these barriers through the
   catalog acknowledgement; serialize late recovery writes with that boundary.
4. Pass the receipt's ticket/report to `core.ack_quiesced` (or
   `ack_recovery_quiesced` for `RestoreCleanup`), then `confirm_quiescence`. Retire
   may settle as Paused when admitted work changed the reviewed obligations.
   **Confirm before pruning the core operation result** or beginning another
   lifecycle operation. The receipt and owner remain available on catalog error.

After an ambiguous catalog write, reload the core first. Do not discard the runtime
table. If a stop committed, `confirm_quiescence` can recognize that exact durable
result under the newer session. If it did not commit, `begin_quiescence` accepts the
fresh same-operation action and repeats the worker fence, scoped cleanup, and final
dependency observation. Other owners and their reservations remain retained.

Finish confirming any cleanup-only owner before activating again. For activation
under a newer session (or a newer same-session Enable lifecycle),
call `reconcile_activation` with the fresh action and same plan. It preserves the
owner/reservation/leases, changes generation, and requires worker fencing plus
draining old leases before restoring services. Other owners also need reconciliation
before they regain network authority in the new catalog session. If activation
fails, persist `core.activation_failed` and perform its scoped cleanup action.

After genuine process loss, rehydrate durable service attempts and use
`recover_quiescence` for pending cleanup, or normal activation for a current
Activate action. Both explicitly fence previous workers. Adapters must reconcile
unknown external resources and durable admitted work; empty volatile tables are
not evidence of completion.

## Current qualification and limits

Host tests cover bounded capacity, management headroom, two owners, failed/pending
driver steps, stale actions/evidence, retained leases, offline admission, explicit
Link/queue/ratchet completion, repeated Enable, and before/after-write failures for
activation and cleanup acknowledgements. They use a recording driver, not live
Reticulum teardown. Native composition tests additionally exercise the real
filesystem/key adapter with that recording driver.

The [native owner](../usage.md) supplies a retained worker and the real
filesystem/key adapter, while concrete service drivers remain caller-owned.
Registrations must stage with ingress closed; pause must retire owned endpoints
and wait for admitted work and retained buffers before releasing key ownership.
These contracts do not themselves enforce quotas inside a service or Reticulum
engine. Setting `Capabilities::QUALIFIED` is an adapter assertion about completion
barriers; it cannot establish physical delivery or secure key erasure.

A native same-lane domain adapter may settle an exact live mutation as
`Settlement::RecoveryRetained` after its physical attempt ends and a confirmed
durable domain record owns the remaining recovery. This drains only the live
lease; it does not assert a final domain outcome, allow fresh work, or erase the
domain's outstanding dependencies. Dropping an admission value is never such a
settlement.

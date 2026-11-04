# Storage workers and private files

`prns-storage` supplies a blocking worker and private-file mechanics with
only `std` in its default dependency graph. The optional `redb` feature adds
physical database construction. No feature depends on a domain, protocol,
identity authority, async runtime or bindings package. The caller owns schemas,
transactions, identity binding, recovery and result publication.

`Worker::start(name, capacity, initializer)` waits until the initializer calls
`Inbox::run` or `run_owned`. Create the database and borrowing managers before that call, then
execute each typed job against those managers inside its handler. Their lifetimes
stay lexical; no self-referential database/manager owner is needed. Startup failure
or panic is joined before returning an error. Resources cannot acquire their file
later after a failed start has returned.

`Inbox::run_owned` supports an owner with pending native effects between jobs.
Its single handler receives `Job`, coalesced `Progress`, and one `Closing` event,
plus a standard task context. Poll bounded retained effect slots using that
context; their weak waker consumes no ordinary queue capacity.
`Inbox::progress_waker` supplies that same signal during driver construction,
before readiness. Progress alternates
with accepted jobs, and close is delivered first even when the queue is full.
The loop returns only after close, an empty queue, and the handler's `Drained`
report. Pending cleanup stays owned after caller loss, without another runtime
or thread. Every handler step must be bounded and nonblocking.

Scoped queue drainage covers completed lane handlers. A domain retaining async
effects beyond handler return must also drain its exact native scopes before
releasing an attachment; `ScopeOwner::close` alone cannot establish that fact.

The queue admits at most `capacity` ordinary waiting jobs, one reserved control,
and one executing job. `Worker::control_client()` and
`ScopeOwner::control_client()` lend the separate `ControlClient` capability to a
trusted lifecycle owner. Keep it out of ordinary domain clients. A full ordinary
queue cannot occupy that slot; a second waiting control returns `Full`. Controls
run before ordinary backlog, but alternate with pending ordinary work or progress
without resetting their own alternation. Continuous controls cannot starve either.
They share scoped fencing, drainage, reentrance rejection and failure handling;
they cannot preempt the currently executing handler. This is one bounded lifecycle
slot, with no configurable priority levels. Submission
is nonblocking and returns the unaccepted job on full/closed/reentrant admission.
Accepted work belongs to the worker, independently of a client's result receiver.
Jobs must own their completion channels; the worker must not skip a mutation
because its waiter disappeared. A handler panic closes admission and abandons
queued work, dropping its result senders; the physical close reports the panic and
pending scope drainage reports failure. This is an execution failure, not proof
that an interrupted domain transaction did not commit.

`Worker::client()` borrows ordinary admission. Dropping any client has no shutdown
effect. `Worker::scope()` creates a separately fenceable/drainable client scope:
closing it waits for that scope's accepted jobs while other scopes stay open. The
shared FIFO makes progress for already accepted jobs even when another scope fills
the remaining queue; it does not reserve ordinary admission slots per service.
Bound and page domain work, since a worker cannot preempt an unbounded handler.
Identity and store compatibility checks belong to the attaching domain, not this
queue primitive.

`request_close` fences admission without placing a control message in the queue.
`Worker::close_signal()` and `Inbox::close_signal()` expose that same irreversible
physical fence as a read-only `CloseSignal`. It closes synchronously on explicit
close, owner drop, or worker exit, even while a handler is blocked. It retains no
worker, queue, or physical resources and does not establish that work has drained.
External native effects may use it as an additional restriction on dispatch.
`close` drains accepted jobs, releases initializer-owned resources and joins. Full
ordinary queues cannot reject shutdown. Both physical and scoped close reject
calls from their own worker, and same-worker submission is rejected rather than
permitting a synchronous dependency cycle. User handlers and dropped queued jobs
run outside the queue lock. Observers must likewise be delivered away from this
worker if they can call consumers or wait for other work.

Dropping a scope fences it without waiting. Dropping the physical owner fences
and detaches: its thread continues to own resources until accepted work drains.
`Worker::closed()` returns a cloneable `Completion<E>` future with a nonblocking
`status()` inspection. It observes cleanup without initiating it; call
`request_close()` to fence first. No ambient executor or new shutdown thread is
required. The initializer and abandoned jobs release their resources before the
result is published. Initializers must finish cleanup before returning; terminal
errors describe its outcome and must not conceal unresolved resources.

Dropping an owner or close waiter does not cancel cleanup. Other retained
observers still receive the same terminal success, error or panic. Pending
completion means cleanup remains unresolved and its resources stay with the
worker. Futures release their registrations on drop, reentrant self-waits are
rejected, and executor wakers run outside all locks. One panicking waker cannot
strand other observers. A completion result observes resource release, not an OS
thread join; use blocking `close()` when joining is required. Repeated joins and
observers preserve the original error through `Arc<E>`.

Foreign finalizers can initiate this fence without blocking the UI thread. An
embedding should retain a completion observer when it needs to inspect finalizer
cleanup. Blocking startup and synchronous close still belong on an embedding's
owned blocking execution path when used by an async or foreign caller.

The headless group peer uses this worker. Its database and
`StoreSession` remain on the worker thread; each group `Job` owns domain
transactions and its result channel. No transaction wraps the generic queue.

The `files` module shares create-new private files/directories, validation and
directory durability confirmation. It never repairs permissions or deletes
uncertain state. Domain owners retain publication ordering, stage replacement,
key validation and recovery decisions; physical I/O failures are not reset policy.
Identity key/catalog storage and local group construction use these same helpers.


With `redb`, `database::Owner::create_new(path, options, initialize)` creates a
private file, invokes the selected schema initializer in one immediate write
transaction, commits it, and confirms the file and parent entry. Initialize
identity binding and selected top-level schemas using that supplied transaction;
opening another writer from the callback would deadlock. A failed callback aborts
its transaction and preserves the incomplete file. Errors during commit or later
confirmation can follow durable success, so recovery opens existing state rather
than deleting it or retrying creation.

`open_existing(path, options, validate)` strictly opens a present file and passes
one read transaction to schema validation. It never creates missing service
tables. The explicit cache budget does not claim to bound all allocations or
reserve disk space. `Error<E>` separates filesystem facts, redb errors and the
caller's typed domain error; it does not prescribe a reset. The immediate parent
must be private, and the embedding controls ancestor directories and renames.

The physical owner itself has no thread. Create it inside an existing worker,
then construct managers borrowing `owner.database()`. Retain dependency leases
until those managers and the database drop. Native groups and identity-scoped
storage use this path. Shared physical construction is not by itself proof of
two-service scheduling or identity admission; those belong to concrete adapters.

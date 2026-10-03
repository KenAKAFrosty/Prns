# Hopspot Hub

Hopspot Hub is the desktop and mobile management application for embedded
Hopspots. This standalone workspace contains its deterministic Rust core and a
native PRNS fitting for remembered-device connections and interface inventory,
with a persistent native controller bootstrap and USB Auto attachment.
It is local development on `hopspot-hub`; review slices stay uncommitted until
approved.

## Core organization

`core/src/domain_primitives` owns validated labels, device identifiers, and
enrollment, connection, interface request correlation values, and pairing candidate provenance. It has no dependency on state
machines.
`core/src/state_machines/device_registry` owns the registry's temporal state,
input/output protocol, transitions, and behavioral tests. Primitive validation
tests live with their primitive. Modules remain private behind the curated
exports in `lib.rs`. The `state_machines/interface_inventory` owner collects
interface pages for one connection and publishes complete results.
`state_machines/device_interfaces` owns that collector's lifetime for one device,
using the registry as the authority for its current connection.
`state_machines/usb_pairing_discovery` owns the bounded set of USB pairing
candidates and its explicit clock.

New core work follows these ownership lanes. Add circuitry only when there are
concrete participants to compose. The standalone workspace owns the Rust and
Clippy baseline, and the core explicitly inherits it through `lints.workspace`.

Machine steps use `Result<Outcome, Error>` only for invariant failure paths.
Their ordinary outcomes remain flat inside `Ok`; steps with no invariant
failure path return their outcome directly. In particular, configured capacity
limits, missing records, stale callbacks, and peer-response failures are domain
outcomes, not invariant errors.

`CreateDevice`, `BeginEnrollment`, `BeginConnection`, and `RefreshInterfaces`
return step-specific errors for exhausted identifiers. `AdvanceUsbPairingDiscovery`
returns an error for backward time, and `ObserveUsbPairingAvailability` returns
an error when the observation is ahead of the discovery clock. These errors
preserve existing state and retain the rejected input or device context where
the operation previously returned it. Architecture snapshots show the `Ok` and
`Err` branches separately.

## Device registry

`hopspot-hub-core` uses `no_std` and Pipecircuit 0.0.16 with explicit alloc-backed
storage. `DeviceRegistry` owns one bounded `WarpTable`, reserved at construction.
Device labels contain at most 128 UTF-8 bytes, must contain a non-whitespace
character, and preserve the supplied text. Labels may be duplicated.

Each operation has its own `StepInputOf<DeviceRegistry>` implementation and
exact outcome: `CreateDevice`, `RenameDevice`, `ForgetDevice`, `ReadDevice`,
`ListDevices`, `BeginEnrollment`, `CancelEnrollment`, `FailEnrollment`,
`CompleteEnrollment`, `BeginConnection`, `ConfirmConnection`, and `EndConnection`.
Invoke them through `StateMachine::step`. The `hopspot-hub-prns` crate owns
the participant and physical transport contracts described below.

`ReadDevice` returns an owned `DeviceSnapshot` that survives subsequent changes.
Read and forget outcomes keep bounded state inline, with local Clippy
expectations for variant size differences, to avoid allocating on these steps.
The creation error likewise preserves the rejected bounded label inline, with a
local `result_large_err` expectation so exhaustion requires no new allocation.
`ListDevices::<CAPACITY>` returns IDs in a stack-backed bounded vector, or
`InsufficientCapacity` with the required count. It never returns a partial list.
List order follows table storage and is not a sorting contract. Ordinary steps
allocate no new heap storage. Duplicate target checks scan the bounded table.

A planned record has a stable local `DeviceId`. `BeginEnrollment`
consumes an existing Remote Control attempt ID and its expected target public
identity, then issues an `Enrollment` token. This slice starts at the identified
protocol attempt; discovery, user selection, and obtaining that attempt are
subsequent integration work. Device and enrollment IDs are scoped to one
registry lifetime and are not a persistence or cross-registry address format.

Completion requires the same token and target. The trusted integration must
supply it only after authenticated Remote Control enrollment and required
persistence succeed. These inputs are in-process contracts, not proof objects
suitable for accepting from an untrusted peer. The hub does not duplicate the
Remote Control cryptographic protocol or own private keys here.

Cancellation and failure return the record to planned. Each retry gets a fresh,
nonwrapping generation even when supplied the same protocol attempt ID. Stale
results cannot modify the new attempt. Different devices can pair independently;
a target already bound to a paired record cannot be bound to a second record.
A failed completion leaves its pending attempt intact for explicit settlement.
Failure outcomes preserve the semantic reason; future adapters own additional
transport diagnostics.

Forgetting removes only the local record and returns its previous enrollment
state and connection state, including outstanding tokens and any live link ID.
The caller remains responsible for cancelling physical work and closing links.
It neither revokes target permissions nor erases a controller identity.
Replacement of an existing pairing is deliberately refused until an explicit
replacement operation is designed.

## Remembered-device connections

`BeginConnection` only accepts paired records. Its `Connect` outcome gives the
transport adapter an opaque `Connection` token carrying the local device ID,
fresh nonwrapping generation, and expected target identity. Duplicate begins
report the active attempt or session without starting another connection.
Connection and enrollment generations are independent.

The trusted adapter supplies `ConfirmConnection` only after PRNS authenticates
the expected target on the reported `LinkId`. The registry checks the attempt
and identity before publishing `Connected`. These in-process values do not
constitute authentication proof. Wrong-target confirmation leaves the pending
attempt intact so the adapter can explicitly settle it.

`EndConnection` settles either an attempt or a live session, preserving its
reason and returning the live link when one exists. Snapshots distinguish
`NotConnected`, `Connecting`, `Connected`, and `Disconnected`. Cancellation,
timeout, transport loss, and authentication failure leave remembered pairing
intact. Ending or forgetting a record invalidates subsequent callbacks; an
old callback cannot settle a newer connection, even if the link ID is reused.
Tokens are scoped to their originating registry lifetime.

The registry owns connection bookkeeping and emits a connection intent. The
PRNS fitting executes that intent. Circuit routing, application deadlines, and
automatic reconnect scheduling while active remain integration work. No
configuration is queued or replayed by these steps.

## Interface inventory

Create one `InterfaceInventory::<CAPACITY>` for each confirmed connection. The
caller selects its interface limit explicitly; staging and published data use
separate bounded buffers with no heap allocation. The collector is subordinate
to the registry's connection lifecycle. `DeviceInterfaces` manages that lifetime
through explicit synchronization, described below. A standalone collector's
caller must close it when its connection ends or the device is forgotten.

`InterfaceInventory::with_initial_refresh(connection)` constructs a collector
already receiving its first page and returns the correlated request. It shares
the same generation transition as `RefreshInterfaces`. `ReadInventoryConnection`
reads only the collector's connection token without copying its inventory.

`RefreshInterfaces` issues an opaque `InterfacePageRequest`. Its `request()`
returns PRNS's `RemoteControlRequest::InventoryInterfaces`; `connection()`
identifies the connection on which the adapter must send it. Its `page()`
accessor preserves the exact upstream cursor for the native PRNS API. The
adapter routes the decoded `RemoteControlInterfaceInventory` back with that exact token.
Every continuation returns the next token and cursor. Duplicate begins report
`Busy`; stale pages or failures cannot alter the pending refresh. Tokens belong
to one collector lifetime; do not reuse an old collector's callbacks if a
collector is recreated for the same connection.

`ReceiveInterfacePage` checks ascending interface IDs across pages and the total
capacity. It publishes only after the final page, preserving upstream interface
kinds, modes, link status, enabled state, and counters. An empty final inventory
replaces earlier data. Duplicate or backwards IDs and capacity overflow fail the
refresh; partial data is discarded. This is complete pagination, not a claim
that all remote interfaces were sampled at the same instant.

`ReadInterfaces` returns owned data plus `NotRequested`, `Receiving`, `Ready`,
`Failed`, or `Closed` status. During refresh or failure, any published entries
are the last complete result and must be displayed as such. `InterfaceRefreshFailed`
settles transport, deadline, permission, or decoding failures. Closing returns
any pending request for cancellation, clears all data, and permanently refuses
reuse. A stale-page outcome returns the rejected page to its caller.

The session driver below provides automatic circuit dispatch. Timed polling,
interface names/configuration, live watches, and interface power changes remain
follow-up work.

## Per-device interface lifecycle

Create one `DeviceInterfaces::<CAPACITY>::new(device)` for each device record,
and keep it for that record's lifetime in its originating registry. The owner
holds at most one collector and allocates no heap storage. Do not reconstruct
the owner for the same live connection: correlation tokens belong to that
collector lifetime.

`SynchronizeDeviceInterfaces { registry: &mut registry }` reads the authoritative
record in one synchronous step. Its borrow is temporary; it neither retains the
registry nor changes its state. A confirmed connection starts an initial
inventory and returns `Started { request, link, cancelled }`. Repeated
synchronization for the same connection returns `Unchanged` and preserves the
pending request, published data, and any failure. It does not retry a failed
refresh automatically. A newly confirmed connection replaces the old collector
and returns its pending request for cancellation, even if intermediate
disconnected/connecting states were not synchronized.

An unconnected, connecting, disconnected, or forgotten device returns
`Unavailable { cancelled }` and removes the collector. Another device's
connection and inventory are unaffected. The same `ReceiveInterfacePage`,
`InterfaceRefreshFailed`, `RefreshInterfaces`, and `CloseInterfaceInventory`
steps apply to this owner with their exact existing outcomes. Late callbacks
from removed collectors, older connections, or another device are stale.
Refreshing with no collector returns `Ok(Closed)`; identifier exhaustion remains
the existing typed invariant error.

`ReadDeviceInterfaces` returns an owned inventory snapshot or `Unavailable` for
the bound device. Explicit close is idempotent and retains a closed collector
until its connection changes or disappears; synchronization cannot reopen it
under the same connection token. A new connection is required to resume after
explicit close.

The future circuit must synchronize this owner after relevant registry changes
and before handling inventory callbacks, refresh intents, or rendering its
snapshot. It must settle the returned cancellation before dispatching a new
request. This step supplies lifecycle coordination and dispatch values; it does
not subscribe to registry mutations or perform network IO. USB runtime wiring
and physical cancellation remain integration work.

## USB pairing discovery

USB Auto is the first live transport target. PRNS already owns USB scanning,
handshaking, lanes, and reconnects through
[`UsbAutoHost`](../../prns-interfaces/impls/tokio/src/usb_auto/mod.rs) and its
[native attachment](../../prns-interfaces/impls/tokio/src/usb_auto/host.rs).
The existing [desktop runtime](../desktop/src/desktop/runtime.rs) demonstrates
that attachment. Hub does not introduce another serial protocol.

`UsbPairingDiscovery::<CAPACITY>::new(interface, now)` requires the actual USB
Auto interface ID and the runtime's monotonic time. Its five operations are
`ObserveUsbPairingAvailability`, `AdvanceUsbPairingDiscovery`,
`ReadUsbPairingCandidates`, `SelectUsbPairingCandidate`, and
`ClearUsbPairingCandidates`. All use `StateMachine::step`.

Observation accepts PRNS's `RemoteControlPairingAvailabilityObservation`, which
the engine emits after signature verification and direct-hop admission. Hub
admits only observations from the configured USB interface. It rejects arrivals
later than its clock, already expired observations, and older or equal-time
updates for the same endpoint. Newer observations replace that endpoint's
provenance and expiry, even when the list is full. Other endpoints are refused
when capacity is reached; no existing candidate is evicted.

The runtime must advance discovery to its current time before processing an
observation, rendering a list, or handling selection. Advancement expires
candidates at their deadline and refuses backward time. Snapshots include
`as_of` so their freshness is explicit. Owned snapshots survive later updates
and clearing. Clearing removes the list without changing its clock or interface;
the runtime must stop or drain old events when resetting the discovery session.

Selection rechecks membership and returns the current candidate, preserving its
endpoint, source interface, observation time, and expiry. It does not consume
the candidate, record enrollment, reserve an attempt, or send a packet. The
future circuit will handle this explicit user intent and the registry's attempt
lifecycle. Availability alone never initiates pairing. Presence in this list
means pairing was advertised within its lifetime; it does not guarantee the
device is still plugged in or that pairing will succeed.

The pairing endpoint is ephemeral and is not the remembered authenticated
target identity. Raw USB locators and announcement app data are not interpreted
as device identity or labels. Existing PRNS pairing requires an invitation code;
the planned code-free headless enrollment requires separate protocol work.

This slice implements the deterministic discovery owner. The native bootstrap now supplies USB Auto attachment and controller identity
persistence, and the session driver supplies automatic inventory dispatch.
Routing verified discovery events and initiating pairing remain integration work. The fitting below
implements authenticated connection and inventory for already authorized
targets. It has not opened a physical USB device.

## Native PRNS fitting

`prns/src/participants` owns the closed `PrnsDevice` input/output protocol.
`prns/src/wiring/fittings/prns_device` owns physical link lifetime and its asynchronous
PRNS calls. The deterministic machines remain in the core ownership lanes.
Create one `PrnsDeviceFitting::new(device, node_handle)` per local device in
its originating registry. Its default backend is the actual `PrnsNodeHandle`;
`PrnsInventoryTransport` adds the existing paged inventory operation to PRNS's
connection transport contract and permits deterministic transport fixtures.

The supplied node must already have its controller identity, authorized target
access, route discovery, and intended interfaces configured. Production's first
attachment remains existing USB Auto. The target must grant inventory permission
and advertise that capability with an installed host provider. This fitting does
not create permissions, persist identities, or initiate pairing.

`DuplexFitting::split` exposes incoming and outgoing halves. Outgoing `send`
returns `PrnsDeviceWork` without starting IO. The async driver calls
`complete().await` outside synchronous Pipecircuit routing, then passes that
result into the incoming half. It emits either the participant output or a
fitting invariant failure. The mutable borrow serializes work per fitting;
independent devices can have independent work in flight. The session switchboard
below owns routing into core steps; the fitting never updates core state implicitly.

`Connect` uses PRNS's target resolution, link establishment, and controller
identification. Success returns the exact `ConfirmConnection` for the core;
the driver must check its outcome and close rejected or obsolete successes.
Connecting again with the active token returns `AlreadyConnected`; another
connection generation returns `Busy` until the existing lease is closed.
`Inventory` verifies the active token and local grant, calls PRNS's bounded
paged API, and returns the correlated `ReceiveInterfacePage` plus RTT.
`Close` releases only the matching lease and preserves PRNS's `Queued` or
`NotQueued` settlement. Queue acceptance does not acknowledge peer shutdown.

Stale callbacks, missing leases, denied requests, and native connection or
exchange failures are ordinary flat outputs. Native failures retain their
upstream types. Wrong-device routing, an unexpected resolved identity, and a
stopped connection worker are fitting invariant errors. The session switchboard
settles operational failures into core state while retaining the native diagnostic.
The fitting trusts supplied core intents; the driver must synchronize the
registry and per-device inventory owner before dispatch and callback handling.

A successful lease queues link closure on drop. Explicit close disarms that
fallback, so it attempts closure once. PRNS owns handling a queued closure; a
`NotQueued` result is returned for explicit close and cannot be reported from
Rust's `Drop`. Shutdown must release fittings before stopping the PRNS node.

PRNS connection establishment is not cancellation safe on its own once a link
exists. The fitting runs resolution, establishment, and identification in a
Tokio worker that continues if its awaiting work is dropped. The worker's
abandoned result drops its lease and queues closure; PRNS itself closes a link
when identification fails. Cancellation therefore abandons the result rather
than immediately aborting the wire operation. Keep the Tokio runtime and node
alive for that settlement. This is not a shutdown-drain coordinator, and the
future driver must bound retries while abandoned work settles. Dropping an
inventory future preserves the established lease for later use or closure.
Calling `complete` requires an active Tokio runtime.

The native integration test runs two PRNS nodes over localhost TCP, with real
identities, explicit grants, request codecs, and Hopspot's existing inventory
provider reading actual interface snapshots. It sends results through the
fitting and core collector. TCP is the test carrier; this does not qualify a
physical USB device, a mobile host, or fresh enrollment. The deterministic tests
also verify pagination, permission and transport failures, stale generations,
foreign-device routing, worker panic, and abandoned connection cleanup.
Property tests compare arbitrary command histories with a single-link ownership
model across three connection generations. Kani cannot practically model this
Tokio/PRNS graph; the isolated label proof remains the bounded formal lane.

The fitting's `behavior.rs` inventories its compiled behavior tests. Architecture
snapshots remain with the core state machines, where Pipecircuit visualization
provides source-derived visual expectations.

## Bounded native work

`prns/src/wiring/fittings/prns_device_worker` owns `PrnsDeviceWorker`, a queued
`DuplexFitting<PrnsDevice>` around the existing physical fitting. Its `try_new`
constructor takes the device, backend, and an explicit nonzero queue capacity,
and returns the fitting plus a future to drive. Capacities beyond Tokio's
semaphore bound return `PrnsDeviceQueueCapacityError` before channel creation.
The queue holds at most the configured number of waiting commands, in addition
to one active command. No registry borrow crosses the worker's I/O awaits.

`TransportTo::send` performs no device I/O and returns `Submitted { completion }`,
`Busy { input }`, or `Stopped { input }`. Rejected commands remain owned by the
caller, which can revalidate and retry them. An accepted completion is independent
of the outgoing transport borrow. Await `completion.complete()` and pass its
result through the incoming fitting; the original PRNS output or fitting
invariant is retained. Loss of the executor becomes a typed worker error.

Dropping a pending completion, including a dropped `complete()` future, cancels
queued connect and inventory work before it starts. It interrupts an active
inventory await without closing the authenticated session. Accepted close work
always executes even if its completion is dropped, so abandoning an observer
cannot suppress requested cleanup. These physical cancellations do not settle
core state: apply the matching core transition before discarding a completion,
and retain all noncancelled session events.

An active connect finishes authentication before processing later work. A new
connection remains provisional until `complete()` accepts its result. If its
caller disappears before or after the result is buffered, the worker releases
the new link before taking another job. Dropping a duplicate-connect completion
preserves the previously accepted connection. This acknowledgement closes the
race between completion delivery and cancellation without detaching another
worker task or copying connection state into a second registry.

For orderly shutdown, stop submissions, consume or drop every outstanding
completion, drop the worker fitting, and await its run future while the PRNS
node remains alive. It drains accepted work and releases its remaining lease
before returning. Holding an unaccepted connection completion deliberately
holds that worker at the delivery boundary. Arbitrarily dropping the run future
is not graceful shutdown; PRNS's existing deferred connection cleanup still
requires a live Tokio executor and node. A close remains a local queue settlement,
not a peer acknowledgement.

The localhost integration now authenticates, collects paginated inventory, and
closes through the queued fitting and session switchboard. Controlled tests cover
backpressure, queued and active cancellation, delivery races, constructor bounds,
typed failures, and shutdown drain. Property tests vary queue capacities and
command/cancellation histories across connection generations, comparing physical
calls with an independent ownership model. PRNS and Tokio dependencies make
these integration and property tests the practical lane; the existing bounded
Kani proof remains unchanged. The session driver and supervisor below now provide automatic dispatch and
shutdown ordering for one device.

## Device session routing

`prns/src/wiring/switchboards/device_session` composes the existing registry and
per-device inventory machines through `DeviceSessionSwitchboard::<CAPACITY>`.
Create it once for a device in its originating registry, alongside that device's
PRNS fitting. Its only temporal state is the existing `DeviceInterfaces` owner;
registry and inventory operations still go through their exact core steps.
The board keeps a compiled-test-derived `behavior.rs` inventory.

Call `Switchboard::route` with `DeviceSessionInput { registry, message }`.
Messages are `Connect`, `Refresh`, `Disconnect`, `Inspect`, and a completed
`PrnsDeviceOut`. The registry borrow ends with that synchronous call. The owned
route contains an event, a device/inventory snapshot, two optional commands,
and two optional cancelled inventory tokens. It allocates no transport queue.
Protocol messages keep bounded native outputs inline, with a local Clippy size
expectation, rather than allocating a box per callback.

The runtime must process cancellations before dispatch and send commands in
array order. Each PRNS command goes through the fitting; feed its ordinary
completion back as a `Prns` message and handle the resulting route in the same
way. Preserve events and native diagnostics even when a route also emits work.
An invariant error from either routing or the fitting requires explicit driver
recovery and cleanup; it is not a normal device refusal. Commands are scoped to
this device, registry lifetime, and fitting. Do not dispatch a command that was
superseded by a later user intent; remove matching queued inventory work when
its cancellation token is returned. Physical work stays outside the switchboard.

Connect success confirms the registry and schedules the first inventory page.
A continuation schedules the next page; only a complete inventory is published.
Duplicate connection requests do not start a second link. Refresh preserves
last complete data and refuses overlap. Disconnect settles the registry first,
invalidates inventory, and emits a correlated close. An obsolete success emits
a close for its old token; if registry synchronization discovers a newer session,
its initial inventory command follows that cleanup. The fitting's token checks
prevent an old close from tearing down the newer physical link.

Every route synchronizes inventory before callbacks and again after registry
changes. Returned snapshots therefore reflect that route's settled core state.
`Inspect` can schedule initial inventory when it first observes a connection
confirmed outside this board; it must be routed through the runtime, rather
than called repeatedly inside a rendering function. The two cancellation slots
cover removal before and after a route; a newly created request cancelled in the
same route is suppressed from dispatch. An initial request already fulfilled by
that callback is likewise not sent again.

Connection failures record the general `DisconnectionReason::ConnectionFailed`;
request failures record `InterfaceRefreshFailure::RequestFailed`. Their route
events retain the exact PRNS error and core settlement outcome. These fallback
reasons avoid labelling a busy service or rejected request as a lost transport.
A fitting with no matching physical link ends only the corresponding registry
connection. A busy fitting settles the rejected attempt and closes an obsolete
lease, preserving one still owned by the registry. Foreign-device callbacks are
invariant failures checked before either device's state can change.

Deterministic tests cover dispatch order, duplicate requests, stale callbacks,
forgetting, explicit disconnect, failed refreshes, malformed pages, and bounded
inventory overflow. Property tests drive arbitrary connect/refresh/disconnect/
inspect histories through the real fitting with a controlled backend, compare
physical calls with an independent ownership model, and replay retired callbacks
while checking another device remains unchanged. The native two-node test now
uses this same switchboard to connect, fetch inventory, and disconnect. The
PRNS/Tokio graph still makes proptest the practical stateful verification lane;
the existing bounded Kani label proof remains unchanged.

A process-wide session event loop, enrollment, automatic reconnect, GUI events,
and application-wide settlement of outstanding work remain integration work. No physical
USB qualification is claimed by the localhost test.

## Native controller bootstrap

`prns/src/wiring/controller_installation` owns the installation's
exclusive filesystem lease, PRNS identity bootstrap, and retained-state store.
`ControllerInstallation::open` takes an explicit dedicated state directory.
It creates that directory, restricts its Unix permissions to 0700, opens a new
0600 `hub.lock` without truncating an existing lock file, and acquires the lock
before loading identities or retained state. Its private lock guard explicitly
unlocks on release, so a duplicated or inherited descriptor cannot prolong the
lease merely by remaining open. As with file closure, destructor unlock errors
cannot be returned; the descriptor is then closed. A competing owner receives the
underlying typed lock error. Failed startup releases the lease.

The existing `RemoteControlIdentityDirectory` stores the distinct controller and
target keys beneath `remote_control`; `NodePersistence` owns `retained`.
Malformed identity material is refused unchanged. Missing keys follow PRNS's
load-or-generate policy. Identity origins remain available so callers can
surface generation versus loading. Bootstrap filesystem, lock, identity, and
persistence errors preserve their typed sources and stage; this is an external
I/O constructor, not a core machine step.

`prns/src/wiring/runtime` assembles those resources through `prepare_native_hub`.
Call it inside a Tokio runtime with the installation, a PRNS event callback,
and a shutdown future. It returns `NativeHubRuntime` with the native node
handle, public identities and their origins, attached USB interface, rescan
signal, and a future to drive. Preparation queues the existing `AutoUsb` with
its upstream interface ID, baud, and policy. Scanning starts when the returned
run future is polled. The handle can supply the existing per-device fitting;
its interface snapshots expose the local USB status. Event callbacks retain
PRNS's verified pairing observations and persistence diagnostics without
introducing a second event representation or an unbounded event queue.

The node starts with no inbound controller grants, no host controls, and no
self-announcement. PRNS restores retained authorization when the run future
starts. Resolve the supplied shutdown future and await `run` to let PRNS flush
state and ratchets before releasing the installation lock. Keep the run future
alive while settling outstanding fitting work first. Dropping an unpolled run
future releases the lease; cancelling a running future does not promise a final
flush. This bootstrap does not coordinate session cancellation or persist the
Hub registry's labels and local device IDs.

Tests cover native USB preparation without polling its hardware scanner, then
drive the same assembly with an empty controlled `UsbAutoHost` scanner. They
check explicit rescans, retained authorization and controller identity across
restart, lock ownership through shutdown diagnostics, and terminal persistence
failure. Filesystem properties vary competing acquisition and release histories
while checking a stable identity. These OS and Tokio contracts use integration
and property tests; the existing bounded Kani label proof remains the formal
lane. The native USB entry point currently targets PRNS's desktop host support;
mobile platform attachment and physical MCU qualification remain future work.
There is no new state machine or artificial architecture snapshot in this slice;
both resource owners retain compiled-test behavior inventories.

## Mio session conduction

`wiring/reactors/mio_session` owns a dedicated Mio poll and a single-slot mailbox.
`MioSessionReactor::try_open` returns the reactor and a clonable sender. Submit
UI intents and PRNS completions through that sender. `Submitted` transfers the
message; `Busy` and `Closed` return the original message for caller settlement.
Wake failure preserves its OS error, but the message is already queued: do not
blindly resubmit it. Dropping a sender closes its channel endpoint before waking
Mio; the destructor wake is best effort because it cannot return an I/O failure.

The reactor checks the mailbox before blocking, retains one pending message
until consumed, retries interrupted polls, and propagates other poll errors.
Wake coalescing cannot erase queued messages. It drains accepted input before
reporting that every sender has closed. At most one message is pending in the
reactor and one is waiting in the mailbox; callers retain rejected submissions.
The physical poll parameter keeps readiness faults testable independently of
the session logic.

`wiring/circuits/device_session` owns the registry and reactor. Compose it with
`DeviceSessionSwitchboard` using `Pipecircuit::new`. Each `conduct()` returns one
`MioDeviceSessionTurn::Routed` with the exact route, including events, ordered
commands, cancellations, and snapshot. Dispatch those commands through the
bounded worker and submit completions back through the mailbox. Callers still
settle cancellations before dispatch and preserve rejected work. Conduction
blocks its calling thread while idle; run the conductor on a dedicated thread
when integrated into an asynchronous application.

`SendersClosed` ends mailbox consumption; it does not disconnect a device or
shut down PRNS. Disconnect and drain accepted worker work while the PRNS node
is alive, then perform native shutdown. `into_parts` and `into_registry` recover
the registry. This circuit currently conducts one device; shared multi-device
registry ownership and automatic worker dispatch remain application assembly.

This follows the Reactor/Circuit split in Pipecircuit's Mio examples, using Mio
1.2.3 directly. Published `pipecircuit-mio` 0.0.5 enables a Pipecircuit default
feature whose Roaring requirement conflicts with PRNS's exact version pin.
The hub therefore keeps its existing explicit Pipecircuit feature selection.

Tests compare complete circuit routes against direct switchboard execution,
exercise actual OS wake delivery and sender closure, inject interrupted,
spurious, and failed polls, and compare arbitrary mailbox histories with a
bounded FIFO model. Finite test scenarios enforce reaction and poll budgets
so loops fail by assertion. The native two-node exchange now passes through this
circuit and the worker. Its messages are submitted before conduction so the
integration test does not block Tokio while waiting for network I/O. OS polling
and the PRNS graph remain integration/property-test territory; the existing
bounded Kani label proof is retained. Both wiring owners have behavior
inventories without artificial machine architecture snapshots.

## Automatic device session execution

`wiring/circuits/device_session_driver` owns `prepare_device_session::<CAPACITY>`.
Supply the registry, one device ID, a PRNS inventory transport, and an update
callback. The returned `DeviceSessionRuntime` contains a clonable handle and a
future to drive inside Tokio. The future runs the Mio conductor on a blocking
thread while driving the existing asynchronous worker. Preparation performs no
device I/O; polling the run future begins execution.

The handle accepts `Connect`, `Refresh`, `Disconnect`, and `Inspect`. Its
single-slot mailbox returns `Submitted`, `Busy`, or `Stopped`, retaining rejected
intents. A wake error occurs after successful publication, so blindly retrying
would duplicate an accepted intent. The public handle cannot inject PRNS
completions. Updates expose settled events and snapshots; the driver already
owns dispatch and cancellation. The callback runs on the conductor thread and
must return promptly; application presentation queues must apply their own
bounded delivery policy.

The driver retains at most one observed worker operation and two pending
commands. The worker has one waiting slot. Completion futures wake Mio directly,
including when worker queue capacity becomes available. The conductor never
blocks on an asynchronous operation or polls it in a busy loop. Ready work is
processed before another intent, preventing a stream of inspections from
starving completions. New intents wait while commands remain pending; the
explicit shutdown signal bypasses that admission gate.

Each route cancels matching inventory work before dispatch and preserves command
order. Closing a connection also discards its pending connect or inventory
observer; the worker still finishes active authentication and releases an
unaccepted connection. Existing closes are preserved. Automatically returned
pages drive subsequent inventory requests, and only complete inventories become
ready. Native peer failures retain their existing route events. Routing,
worker, and internal command-capacity invariants stop conduction and return the
registry with the exact failure for recovery.

`shutdown()` stops admission and wakes the conductor even when the intent
mailbox is full. Shutdown supersedes queued UI intents and undispatched device
commands, routes disconnect, and drains cleanup. Dropping the last handle also
starts shutdown after accepted mailbox input is consumed. The run future returns
only after the conductor exits and the worker drains. Its `DeviceSessionExit`
returns registry ownership and conduction settlement. Observer panics are
reported as Tokio join errors after worker drain; they do not return the
registry. Ordinary shutdown and invariant exits do return it.

Dropping the run future requests stop and cancels its async worker; it is not
orderly shutdown and does not await cleanup. Keep polling the future after
requesting shutdown, and keep the PRNS node alive until it returns. Active
connection operations still use PRNS's own deadlines. Configurable Hub operation
deadlines, timed refresh, and automatic reconnect remain subsequent work.

Tests cover automatic pagination, refresh, cancellation, reconnect, bounded
admission, queue ordering, retained readiness, typed failures, and observer panic.
Property tests compare arbitrary intent histories with an independent physical
connection/inventory call model. OS polling and Tokio ownership use integration
and property tests; the existing bounded Kani label proof remains the formal lane.

## Session and native shutdown supervision

`wiring/session_supervisor` owns `supervise_device_session`. Supply a
prepared session, the native node's run future, a one-shot node-stop callback,
and the application's shutdown future. With `prepare_native_hub`, that callback
resolves the shutdown signal originally supplied to the native bootstrap.

On an application shutdown request, supervision stops session admission and
continues polling both owners while the session disconnects and drains its
worker. Only then does it signal native shutdown and await the node's result.
If the session ends first, it requests native shutdown immediately. If the node
ends first, it stops and settles the session while retaining the node result.
An already exited node cannot perform additional cleanup; its failure remains
visible rather than being replaced by a successful local stop.

`SupervisedSessionExit` preserves the triggering reason, session result, node
result, and any explicit shutdown-wake error. Shutdown is local settlement and
PRNS persistence completion, not acknowledgement from the remote MCU. A stuck
observer or backend can delay orderly shutdown; the supervisor does not impose
an arbitrary forced-termination deadline.

Tests cover all three initiating events and prove authentication/close drain
precedes the node-stop callback. The real localhost two-node exchange now uses
automatic session execution and this supervisor to authenticate, collect
inventory, disconnect, and stop its controller. Native bootstrap tests retain
coverage of persistence flushing and installation lock lifetime. These two
wiring slices retain behavior inventories; no new deterministic core machine or
artificial architecture snapshot was introduced.

## Verification

From this directory, run:

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path verification/kani/Cargo.toml --all -- --check
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cargo build --locked -p hopspot-hub-core --lib --no-default-features
cargo llvm-cov clean --workspace
cargo hub-coverage
CARGO_INCREMENTAL=0 cargo hub-mutants --file 'prns/src/wiring/reactors/**' --file 'prns/src/wiring/circuits/**' --file 'prns/src/wiring/fittings/prns_device_worker/**' --file 'prns/src/wiring/runtime/**' --file 'prns/src/wiring/session_supervisor/**'
cargo kani --manifest-path verification/kani/Cargo.toml --lib --output-format terse
```

The [Cargo aliases](.cargo/config.toml) follow Pipecircuit's
[protocol coverage alias](https://github.com/KenAKAFrosty/pipecircuit-scaffold/blob/ee5470f9287505c4a7e7d449689062258a544d9d/.cargo/config.toml)
and [pre-push checks](https://github.com/KenAKAFrosty/pipecircuit-scaffold/blob/ee5470f9287505c4a7e7d449689062258a544d9d/.githooks/pre-push).
The [mutation configuration](.cargo/mutants.toml) follows its
[cargo-mutants configuration](https://github.com/KenAKAFrosty/pipecircuit-scaffold/blob/ee5470f9287505c4a7e7d449689062258a544d9d/.cargo/mutants.toml),
without the WebSocket-specific exclusions. No hooks are installed by this slice.

Coverage requires zero uncovered production functions, lines, or regions. Test
files, architecture/behavior snapshots, and proof harnesses are outside that
denominator; no production source is excluded. This is source coverage, not
exhaustive branch or application verification. The unfiltered mutation alias covers both
workspace crates except proof bodies. For incremental candidates, filter to
changed and new production owners plus affected existing owners; the command
above shows this slice's scope. File filters include untracked source that a Git
diff would omit. Every selected mutation still runs the full workspace test
suite. Report the scope and prior full baseline explicitly. Run the unfiltered
`cargo hub-mutants` for changes with broad impact on contracts, dependencies,
or test infrastructure, or when requested. Require zero missed or timed-out mutants and report caught and
unviable counts separately.

Mutation testing runs in place so the existing PRNS path dependency resolves
normally. Run it without concurrent edits or checks; cargo-mutants restores each
source mutation. Disabling incremental compilation bounds build-cache growth
during the many rebuilds. Its reports live in `target/hub-mutants/mutants.out`. Coverage
data remains in `target/llvm-cov-target`. Tool requirements are `cargo-llvm-cov`
with LLVM tools, `cargo-mutants`, and an installed Kani toolchain.

`architecture.rs` derives its machine graph from Rust source using
`pipecircuit-visualization`. `behavior.rs` inventories the compiled tests; it is
not itself evidence that they passed. To accept an intentionally reviewed
snapshot change in this nested workspace:

```sh
CARGO_WORKSPACE_DIR="$PWD" UPDATE_EXPECT=1 cargo test --workspace --lib remains_reviewable
```

Tests use PRNS's test-support feature to construct protocol attempt IDs; that
feature enables `std` in the test dependency graph. Production builds omit it.
Allocation-error and exhausted-ID translation are checked with synthetic storage
outcomes, without provoking host memory exhaustion or executing 2^64 inserts.
The registry advances enrollment through one borrowed row, keeping validation
and mutation under the same owner without an intervening fallible lookup.

The Kani package in `verification/kani` imports the actual label primitive source
and proves preservation/rejection for every two-byte ASCII input with unwinding
assertions enabled. The bound does not cover all Unicode or label lengths;
Unicode property tests complement it. Registry properties exercise arbitrary
cancel/retry histories and compare record creation/removal with an independent
map model. Connection properties exercise arbitrary connect/confirm/end histories,
reject all retired tokens, and check another device remains unchanged. Maximum
generation is covered by a deterministic exhaustion test. These stateful checks
use proptest because importing the full PRNS graph still blocks Kani.
Interface inventory properties vary sorted interface sets and page boundaries,
roundtrip real PRNS request/response bytes, and compare the complete published
result with the original entries. Deterministic tests cover invalid ordering,
overflow, failure recovery, exhaustion, and reconnect invalidation.
USB discovery fixtures construct signed availability packets, run PRNS's
ingress and deferred signature verification, and feed genuine observations to
the machine. Its property test compares arbitrary arrival, expiry, selection,
and clear histories with an independent bounded model. The same PRNS dependency
restriction makes property tests the practical lane for this machine; no new
Kani result is claimed for discovery.
Device-interface lifecycle properties vary completed and pending inventories,
explicit close, and intermediate synchronization across reconnect histories.
They reuse the same physical link ID while rejecting every retired request.
Deterministic cases cover wrong-target confirmation, two-device isolation,
forgetting and registry row reuse, pagination, refresh failure, and idempotence.
These PRNS-dependent stateful checks also use proptest instead of a new Kani
harness; the isolated label proof remains the formal verification lane.
The isolated proof package avoids an existing `prns-core` Kani
compilation failure in its request-set proof (a `u32` shift by 32); the main core
crate's Kani lane is not claimed to pass.

Both workspaces retain lockfiles. The native integration test requires localhost
socket access. No physical device or non-host platform is qualified by these
checks.

## Next boundaries

USB discovery now exposes an explicit selection step; runtime pairing dispatch
still requires that user intent. Remembered devices will
reconnect automatically without replaying configuration commands. Headless
first enrollment will use an unowned boot window, direct-only admission,
code-free automatic approval, and owner authority. Network-driven interface
status and power controls follow; a change affecting the requesting controller's management
interface will need a device-owned confirmation deadline and rollback.

Per-installation identity, native USB bootstrap, and bounded physical work are
in place, together with automatic per-device dispatch, cancellation, and native
shutdown supervision. Next, connect verified USB discovery and enrollment to
this driver, persist Hub device records, and add reconnect/deadline policy. Controller sharing, explicit
device replacement, firmware installation, clusters, and relationship views
remain later capabilities.

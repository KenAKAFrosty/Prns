# Host runtime

`HostRuntime` owns the PRNS runtime thread and the canonical command, upload and
inspection lanes. `HostSessionEvents` owns bounded application/diagnostic
queues, lifecycle state, readiness and received resources. C and UniFFI both use
these owners; their adapters contain only transport representation and foreign
object ownership.

The crate has no default feature selections. `connections` adds the optional
saved-profile store and host binding; ordinary host construction and RemoteControl
remain available without it. `android-platform` selects the callback bridge,
and `apple-platform` selects the Apple LAN adapter. Protocol, interface and
platform support must still be queried through the actual host's `BackendInfo`.
This crate owns host execution. An embedding coordinates any domain owners that
share it and selects its own installation policy.

## Foreign and Rust clients

Use `owner::OwnedSession::open` for asynchronous startup. Retain that owner in
native composition when background work must survive JavaScript replacement.
`OwnedSession::client` returns a cloneable `HostClient` with commands, snapshots,
uploads and event access; a borrowed client cannot stop the composed owner.

Use `OwnedSession::client_scope` when a composition must revoke external admission
before host shutdown. Retain the `HostClientScope` and return its ordinary
`HostClient` clones. `fence` and scope drop synchronously prevent new commands,
uploads and typed RemoteControl operations at their actual queue insertion.
Already admitted work, upload handles and event streams retain their existing
continuations; snapshots and public-key queries remain readable until host stop.
Internal service cleanup can keep a separate unrestricted `OwnedSession::client`.
A client future that has never been polled has not been admitted.

Scoped clients reject `service_client` and arbitrary `protocol_operation` with
`RestrictedCapability`: exporting a raw protocol handle would bypass revocation.
The existing host-contract generator derives typed RemoteControl native methods
and foreign projections from the same core trait inventory. `HostClient::events`
returns `HostSessionObserver`, the hub's read-only lifecycle/stream view. Code that
needs to publish must retain its owning `HostSessionEvents`; borrowed clients
cannot mutate lifecycle state.

Command and snapshot futures need no caller Tokio runtime. They wake from the
host runtime without a blocking worker per call. Cancelling a command wait does
not cancel an admitted operation. `HostRuntimeUpload::write_async` changes its byte
accounting only after channel admission; cancelling a capacity wait leaves it
unchanged.

Foreign finalizers must never own/drop `HostRuntime` directly. Its synchronous
`stop_result` joins the worker. `OwnedSession` schedules that operation on its
lifecycle executor, and all stop waiters receive the completed result. The host
holds an exclusive `.prns-host.lock` file lock for persisted state until the
runtime has joined. Do not unlink that lock file while another owner may open
the directory.

## Owned transport attachments

`OwnedSession::attachment_scope` gives a native service attachment ownership
without host stop authority. Its `attach` and `detach` futures use canonical
interface configuration and IDs on the existing command lane. The same attachment
map records the claim: ordinary clients cannot recreate a claimed endpoint or
detach its interfaces, and another scope cannot adopt an observed ID. Conflict
checks use canonical planning defaults and endpoint/device identity before opening
resources; DNS spelling and bitrate/routing changes do not bypass the claim.
Prepared native resources with no configuration are conservatively reserved by
kind, and trusted embedding providers must still return fresh resource ownership.

`fence` stops fresh attachments synchronously while preserving existing network
resources for service drainage. `close` fences before returning its future, then
uses the independent cleanup lane and waits for actual retirement. Dropping the
scope requests that same retained cleanup; dropping a waiter does not abandon it.
A completed cleanup failure can be retried explicitly. `RetirementPending`
identifies a partial attachment whose failed retirement still owns an exact ID;
keep it until owner-qualified `detach` or scope cleanup succeeds. No automatic
reattachment follows an uncertain failure. `stopped` observes the session's joined
shutdown without requesting it. These futures need no ambient Tokio runtime.

## Retained RemoteControl exchanges

`remote_control::RemoteControlOperations` is an optional process-local owner over
ordinary `HostClient` admission. Choose a nonzero capacity up to 64, submit a
canonical request to a target or an existing link, and retain the returned ID.
`operation`, `operations`, and repeatable `wait` return the canonical result after
waiter loss. The bounded host task owns execution; this retainer creates no runtime,
actor, database, or retry loop. Retain the object in native composition to keep
results across JavaScript reloads or host replacement. It is not crash persistence.

A record stores destination, request kind, and typed state/result; it never stores
the credential-bearing request. Credentials exist only in the accepted task and
its ordinary protocol send. There is no automatic replay. `Pending` means request
submission has not happened. `Submitted` is the local request-queue boundary, not
proof of peer delivery. `NotSubmitted` requires failed admission or a positive
engine rejection. `Completed` preserves the canonical response or peer-reported
protocol error; it does not claim every remote mutation succeeded. Lost or malformed
responses and shutdown after submission become `OutcomeUnknown`. Inspect canonical
remote status/revisions before deciding what to do with an uncertain write.

`cancel` linearizes with submission: cancellation before submission prevents the
request; after submission it reports `AlreadySubmitted` and keeps observing. It
does not claim joined transport teardown. In-progress connection setup still has
a bounded native owner and closes its temporary link. A caller-supplied link
remains caller-owned. Host shutdown drops queued work and joins active work, with
both paths publishing their distinct retained state. Scope fencing denies fresh
operation admission while preserving previously accepted continuations.

Terminal results consume capacity until `retire`. Pending/submitted work cannot be
retired; no result is silently evicted, and IDs are never reused by the owner. A
cancelled result can be retired while bounded connection cleanup still runs; the
host admission permit continues to bound that task. UniFFI exposes this same owner
and generates all retained values from these native declarations.

## Event ownership

`HostServiceScope::new_restricted` accepts a native `HostDispatchGate` bound
to the embedding's exact owner. It adds denial before reservation/admission and
again on the actual queued protocol dispatch, without replacing authorization.
Gate checks run outside scope locks and cannot invoke foreign callbacks. Closing
the gate preserves already-owned operations and LinkClosed routing; the existing
scope fence and endpoint barrier still prove cleanup. A restored owner constructs
a fresh scope/gate rather than reopening the old generation.

Native services sharing a host can use `service_scope::HostServiceScope` to
bind destination/Link access, bounded admission and actual-operation drainage to
one local identity generation. Feed it lifecycle events even after fencing new
work. The node's `ServiceEndpoint` API stages registration with ingress closed,
then supports explicit activation and retryable scoped retirement. These are
trusted native integration primitives, not caller authentication or a substitute
for an embedding's catalog admission. Buffer or Link completion alone does not
prove that a remote application accepted a domain operation.

Each event lane has one active `HostRuntimeEventStream`. Await `ready` without
consuming, then call `try_next` for a bounded synchronous drain. Cancelling
readiness cannot strand an event in a dead promise. `close` invalidates the
lease and releases its claim, even while an old readiness future remains alive;
the old stream cannot release a replacement consumer's claim when it drops.
Diagnostic gaps remain with the lane across consumer replacement.

A returned resource transfers with its event and can outlive the lane/session.
Resource chunk reads require a positive maximum length. Foreign adapters enforce
their maximum returned chunk size before calling the shared reader.

`Readiness` supplies optional native callbacks and runtime-independent async
notifications. Callbacks must be short signals, must not unregister themselves,
and must not access the stream synchronously. Unregister quiesces in-flight
callbacks before a C adapter releases its context.

## Host embedding

`HostEmbedding` supplies opt-in RemoteControl configuration, prepared transport
attachments, a plan runtime context (including a pre-existing BLE identity), owning
native event dispatch, and authenticated accepted-announce observation.
The prepared transport and callback hooks are native Rust integration points.
Their exact availability, together with destination public-key lookup, is recorded
in the generated [native extension inventory](../../native-extensions.generated.md).
The inventory links public declarations and distinguishes native Rust, native
UniFFI, C, N-API and browser/cooperative projections. Authenticated announce
observations have no foreign projection yet; announce diagnostics do not provide
that authenticated service contract.
The UniFFI/RN transport also exposes `remote_control_config` as an explicit,
fingerprinted extension alongside canonical `HostConfig`/`HostCommand` configuration.

`HostServiceClient` exposes the existing public Rust protocol handle and clock
without an owning node or stop authority. Return every prepared interface from
`prepare_interfaces` as a `HostPreparedAttachment`, so canonical inspection
and detach account for it. A `Registered` attachment supports platform types
such as prepared BLE whose status owner differs from `AttachedInterface`.

The default `ApplicationEventDispatch::Queue` preserves canonical event queues.
Opt-in RemoteControl observations use that same bounded application lane and its
single consumer. The engine retains live pairing attempts within its existing
limits; confirmation events contain public facts and an attempt ID. Approval and
rejection use the existing protocol controls, which reject expired or closed
attempts. Dropping a public confirmation view does not lose the engine's authority.
C, NAPI, and cooperative transports do not advertise this native extension.

`HostRemoteControlConfig` resolves caller-selected controller and target identity
sources and optional initial grants. It adds no application identity paths,
capability choices, or persistence policy. The same runtime service restores and
flushes authorization when the caller chooses persisted host state.

A composed Rust service may explicitly choose `EmbeddingCallback`; its owner
reserves the application lane and routes application messages only to the native
callback. This callback must enforce its own service admission bounds. Consumers
cannot simultaneously claim the same lane through JavaScript. Native callbacks
receive the owned event, allowing move-only protocol replies to remain intact.

`HostSessionEvents::wait_terminal` observes unexpected runtime termination
without claiming either lane. It is not a join acknowledgement: the owning
composition still drains its services and awaits `OwnedSession::stop` before
resetting or replacing persistent state. Joined stop preserves typed runtime and
persistence failures for every waiter, so failed authorization flush cannot be
mistaken for successful shutdown.

## Platform ownership

Android's safe `platform::android` module reserves one callback bridge per live
Bluetooth generation. Its supervisor retains that reservation until the actual
transport future is dropped. A consuming Android image supplies its JNI adapter;
the host itself retains its unsafe-code prohibition. Callers must stop old Kotlin
callback pumps before preparing their next generation.

Native protocol extension jobs share admission bounded by `pending_commands`.
Cancelling a foreign waiter does not cancel an admitted job. The host owns their
join set, aborts and joins them during shutdown, and only then publishes completion.


## Pairing discovery without an event consumer

`HostClient::remote_control_pairing_discovery` reads authenticated availability
captured before Queue or EmbeddingCallback routing. It never claims or drains the
application stream. Each result carries a unique native host generation and the
runtime clock's observation time. The captured client cannot silently follow a
replacement host, and queries fail once its host stops. Revoking only a borrowed
mutation scope preserves this read access.

The native cache starts without allocated entries and retains at most 64 endpoint
observations, each with the protocol-bounded public application data. Newer
observations replace the same endpoint; duplicate and older observations do not
extend its deadline. Expired entries are excluded at query time using the runtime
clock. At capacity the oldest observation is replaced unless the incoming observation
is older still. Cumulative capacity omissions are reported as `evicted` in the
snapshot, including incoming older entries omitted in favor of newer entries. This is a bounded recent view, not a complete
inventory. A peer can close its pairing window before the observed deadline, and
an announcement grants no pairing or management authorization.

`HostClient::initiate_remote_control_controller_pairing` exposes the existing core
high-level initiation algorithm through bounded native admission: establish the
pairing link, begin the exchange, and apply the algorithm's cleanup on failure.
The caller passes the observed endpoint/deadline and invitation code. The engine
validates its actual state and deadline; neither discovery nor UI state authorizes
approval. Accepted initiation survives waiter loss; a new controller-pairing
snapshot recovers current confirmation. The lower-level begin API remains
available to consumers that already own a link.

## Owner checks

From the repository root:

```sh
python3 validation/run.py run --suite host-native-session
python3 validation/run.py run --suite host-native-connections --suite host-native-connections-clippy
```

The first suite covers host sessions and the Android callback bridge on the
host. The connection suites cover the optional profile owner, exact saved/applied
state and retained cleanup. Generated transports have their own
[foreign checks](../../bindings/uniffi/README.md#verification). Loopback and
injected-provider tests do not establish device permissions, radio delivery or
mobile background behavior.

`HostClient::remote_control_controller_pairing_status` atomically reads the live
ceremony and its most recent terminal result. The engine retains one result until
a new invitation is admitted or the host is closed; inspection does not consume
it. `Paired` is recorded only after the exact target access is persisted. A view
can reconcile the returned target hash with saved target inventory. `Idle` alone
never establishes success, rejection or expiry.

Cancellation carries the displayed subject: the exact exchange context before an
offer, or its transcript-derived attempt ID and context afterward. It rejects
stale subjects and cannot undo approval or persistence. Failed links and requests
remain failures, distinct from explicit cancellation. These observations and
cancellation are additive; the existing live-snapshot and approval APIs retain
their contracts.

# Shared native host

`NativeHost` owns the PRNS runtime thread and the canonical command, upload and
inspection lanes. `NativeSessionEvents` owns bounded application/diagnostic
queues, lifecycle state, readiness and received resources. C and UniFFI both use
these owners; their adapters contain only transport representation and foreign
object ownership.

## Foreign and native clients

Use `owner::OwnedSession::open` for asynchronous startup. Retain that owner in
native composition when background work must survive JavaScript replacement.
`OwnedSession::client` returns a cloneable `HostClient` with commands, snapshots,
uploads and event access; a borrowed client cannot stop the composed owner.

Command and snapshot futures need no caller Tokio runtime. They wake from the
host runtime without a blocking worker per call. Cancelling a command wait does
not cancel an admitted operation. `NativeUpload::write_async` changes its byte
accounting only after channel admission; cancelling a capacity wait leaves it
unchanged.

Foreign finalizers must never own/drop `NativeHost` directly. Its synchronous
`stop_result` joins the worker. `OwnedSession` schedules that operation on its
lifecycle executor, and all stop waiters receive the completed result. The host
holds an exclusive `.prns-host.lock` file lock for persisted state until the
runtime has joined. Do not unlink that lock file while another owner may open
the directory.

## Event ownership

Each event lane has one active `NativeEventStream`. Await `ready` without
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

## Native embedding

`NativeEmbedding` supplies opt-in RemoteControl configuration, prepared transport
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

`NativeServiceClient` exposes the existing public Rust protocol handle and clock
without an owning node or stop authority. Return every prepared interface from
`prepare_interfaces` as a `NativePreparedAttachment`, so canonical inspection
and detach account for it. A `Registered` attachment supports platform types
such as prepared BLE whose status owner differs from `AttachedInterface`.

The default `ApplicationEventDispatch::Queue` preserves canonical event queues.
Opt-in RemoteControl observations use that same bounded application lane and its
single consumer. The engine retains live pairing attempts within its existing
limits; confirmation events contain public facts and an attempt ID. Approval and
rejection use the existing protocol controls, which reject expired or closed
attempts. Dropping a public confirmation view does not lose the engine's authority.
C, NAPI, and cooperative transports do not advertise this native extension.

`NativeRemoteControlConfig` resolves caller-selected controller and target identity
sources and optional initial grants. It adds no application identity paths,
capability choices, or persistence policy. The same runtime service restores and
flushes authorization when the caller chooses persisted host state.

A composed native service may explicitly choose `NativeCallback`; its owner
reserves the application lane and routes application messages only to the native
callback. This callback must enforce its own service admission bounds. Consumers
cannot simultaneously claim the same lane through JavaScript. Native callbacks
receive the owned event, allowing move-only protocol replies to remain intact.
This embedding mode preserves the original move-only protocol replies for the
native application owner.

`NativeSessionEvents::wait_terminal` observes unexpected runtime termination
without claiming either lane. It is not a join acknowledgement: the owning
composition still drains its services and awaits `OwnedSession::stop` before
resetting or replacing persistent state. Joined stop preserves typed runtime and
persistence failures for every waiter, so failed authorization flush cannot be
mistaken for successful shutdown.

## Platform ownership

Android's safe `platform::android` module reserves one callback bridge per live
Bluetooth generation. Its supervisor retains that reservation until the actual
transport future is dropped. JNI is isolated in `prns-react-native/platform/android`;
the native host retains its unsafe-code prohibition. Callers must stop old Kotlin
callback pumps before preparing their next generation.

Native protocol extension jobs share admission bounded by `pending_commands`.
Cancelling a foreign waiter does not cancel an admitted job. The host owns their
join set, aborts and joins them during shutdown, and only then publishes completion.

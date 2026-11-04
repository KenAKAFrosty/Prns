# Standalone group bindings

This workspace exposes the ordinary local group client to native consumers.
`prns-groups-uniffi` supplies only construction, key-capability retention and
cleanup observation. Core identifiers/errors, service records and native
requests/results/client methods are generated from their owning declarations;
there is no second domain model or method-forwarding facade.

The `image` member builds `prns_group_services`, containing the group core,
records, native client, canonical store limits, owner and identity-key namespaces. Generate all namespaces
from that image into one Swift module. Create keys through that same module;
objects from separately loaded native images have separate handle registries and
must not be exchanged. An embedding can instead depend on this rlib and aggregate
these namespaces in its own single native image.

The selected profile disables native text and networking features. It requires
no host, identity catalog, ambient executor, React Native or Node tooling. Tokio
supplies synchronization primitives only. The lower group owner remains usable
from Rust without depending on identities; this narrow factory derives its
public identity binding from an opaque `KeyLease` and retains the native lease
until the worker releases its manager and database.

## Ownership and recovery

`GroupLocalOwner.createNew` creates a new private group database with a managed
catalog. `openExisting` requires the existing catalog and matching key capability.
Both require explicit queue/cache budgets. The owning Rust constructor enforces
the resource bounds and filesystem contract before readiness. Construction is
blocking: invoke it off the UI thread. See the [native owner contract](../../usage.md)
for supported platforms, permissions, partial creation and durability limits.

`client()` returns the ordinary generated `Client`. Its immutable `GroupRequest`
and separately supplied `GroupExecutionContext` are the same types Rust callers
use. Retain a request unchanged across an ambiguous outcome. Exact retained
results can recover with unknown time; fresh mutations require trusted clock
evidence and the captured original group scope, including its exact store
incarnation. Generic records preserve opaque
application bytes, provenance, insertion cursors and delivery evidence.

Conversation and invitation queries use `GroupPageLimits` and an exclusive
cursor. Follow the returned cursor even after a short page; `next == nil` marks
the current range exhausted. Summaries omit the roster. Query exact conversation
detail and member pages separately; member pages retain their control reference
so callers can detect membership changes between reads.

`shutdown()` fences admission synchronously and returns `GroupClose`. Waiting observes
cleanup on the existing worker; repeated waits retain the same result. Dropping
an observer does not cancel cleanup or accepted work. Dropping the owner fences
it too. Borrowed clients do not prolong admission. Await cleanup before reopening
the database or expecting the worker's key lease to release. Cleanup is not an
assertion that every accepted domain operation succeeded.

The generated `Client.subscribe()` and `GroupObservation.next()` follow the
same bounded observation contract as Rust: subscribe before the initial snapshot,
refresh ordinary queries after each coalesced hint, and recreate the subscription
after owner closure. A hint may follow an uncertain write and is not evidence of
success. Owner closure ends observation before physical cleanup may finish; await
`GroupClose` to confirm cleanup.

Retained-operation methods and models also come from the ordinary client. This
factory selects only the managed catalog; retained submission requires an
embedding that explicitly initializes the retained schema and supplies the
canonical authorization adapter. The presence of generated methods does not
silently enable that facility.

The canonical `GroupApiError` distinguishes input/rejection/retry failures from
busy, storage-capacity and uncertain/unavailable outcomes. Native lower-level
APIs keep their richer backend diagnostics. Checked foreign converters reject
wrong-width identifiers, zero incarnations and invalid intervals with this same
typed error; they do not bypass domain constructors. Startup I/O/database errors
may follow durable effects and never imply that deleting or recreating the file
is safe.

## Qualification

From the repository root:

```sh
python3 prns-groups/bindings/uniffi/tests/check_profile.py
# macOS with Xcode command-line tools; no npm or RN generator required:
python3 prns-groups/bindings/uniffi/tests/check_swift.py
# macOS, Java 17+ and Gradle 9.3.1; no Android SDK or npm:
python3 prns-groups/bindings/uniffi/tests/check_kotlin.py
```

The first check verifies the actual dependency closure and runs adapter tests.
The Swift check builds the image, generates its real interfaces through the
shared UniFFI tooling, checks exposed symbols, compiles a separate consumer and
executes it. It exercises key retention after foreign wrappers are finalized,
canonical creation/replay, typed native lifting failures, record/feed queries,
owner-published change observation,
shutdown fencing, repeated cleanup observation, and durable reopen. These are host
integration tests; they do not qualify phone background operation or networking.

The optional `uniffi-bindings` features on core/service/native declarations are
absent from ordinary default and portable consumers. The independent Rust local
consumer and embedded core checks remain separate gates. Generated outputs are
build artifacts; no generated copies are committed here.

The Kotlin check generates every namespace from the same independent image,
compiles them with a separate JVM consumer, and executes key creation, ordinary
client queries and retained shutdown against the native library. It uses pinned
Kotlin 2.4.10, JNA 5.18.1 and coroutines 1.10.2. `PRNS_KOTLIN_GRADLE` can select an
existing Gradle executable; `PRNS_KOTLIN_OFFLINE=1` requires its cached dependencies.
The foreign `shutdown()` name is deliberate: Kotlin's generated `close()` only
releases the foreign wrapper through `AutoCloseable`. The ordinary Rust owner's
`LocalOwner.close()` remains unchanged. No Android or Expo runtime is involved.

# General-purpose Expo/RN SDK: UniFFI implementation plan

Status: implemented as an independent Expo SDK with app composition above the
shared host. Bounded Android/iOS ownership, Hermes reload and retained-data
checks have passed for the default SDK and app aggregate, with different session
ownership semantics. The [implementation record](react-native-sdk-implementation.md#current-qualification)
links their evidence and remaining release/background gates. Do not read the
original plan below as a current list of unimplemented work.

This plan audited `e39eb2a1c6eff87875662a42c3ea85828a26b9b2` on 2026-09-22.
Its baseline gaps and investigation results describe that source. The exit
criteria remain the acceptance requirements; bounded device checks do not close
all lifecycle or distribution criteria. See the
[architectural investigation](react-native-sdk-investigation.md) for the original
ownership audit.

## Settled direction

Use Expo, the existing pinned UniFFI/JSI generator, the canonical host contract,
and the shared `NativeHost`. The public SDK is independent of the PRNS app.
The app retains its Rust services and product composition above public host
APIs. It need not route durable native work through JavaScript.

```text
PRNS app JS ──────────────> general Expo/RN SDK
    │                              │
    v                              v
generated app/service bindings   generated host bindings
    │                              │
    v                              v
app composition + LXMF ───────> public shared host session
                                   │
                                   v
                              NativeHost / PRNS
```

The native components in this diagram are linked into one selected Rust
library. A standalone SDK consumer uses the SDK's default library; this app
uses an aggregate library containing the same SDK crate plus its extensions.
Expo integration is required. A separate non-Expo implementation is out of scope.

The significant correction to the earlier proposal is that a thin wrapper is
the **end state**, not the whole implementation. Some host mechanisms must first
be extracted from the C adapter, and several app requirements need general host
APIs. Otherwise the UniFFI wrapper would reproduce those mechanisms.

## Baseline gaps found before implementation

| Gap | Source evidence | Required resolution |
| --- | --- | --- |
| The reusable native session is incomplete. | [NativeHost](../../prns-host/impls/native/src/lib.rs) accepts a `NativeEventSink`. The [C adapter](../../prns-host/abi/c/src/lib.rs) owns `Shared`, `HostPublisher`, stream readiness, lifecycle state, and received resource storage. | Move the language-neutral implementation into shared native Rust code, and make C and UniFFI consume it. Do not copy the C capsule into another adapter. |
| The existing application generator covers only part of the contract. | The baseline app's `tools/generated-bindings/host_contract.py` (since removed) traverses the `HostSnapshot` record graph. | Extend the common schema pipeline to all SDK inputs, outputs, unions, failures, and ownership operations. |
| Some public envelopes lack a complete schema definition. | [HostConfig](../../prns-host/core/src/config.rs), [lifecycle](../../prns-host/core/src/lifecycle.rs), [limits](../../prns-host/core/src/limits.rs), and the generator's [model](../../tools/repo/host_contract/model.py). `HostOptions` and several raw operation types are recognized without being ordinary fully specified records. | Close these schema gaps before claiming complete generation. Reconcile `HostConfig`, JS creation options, required capabilities, and lifecycle/results instead of creating another handwritten definition. |
| Directly exporting `NativeHost` would put blocking work in foreign calls/destructors. | `start`, `snapshot`, command `wait`, `stop`, and `Drop` in [native lib.rs](../../prns-host/impls/native/src/lib.rs). `Drop` calls `stop`, which joins a thread. | Foreign objects hold nonblocking leases on a native owner. Use async readiness/completion, and keep creation, I/O, and joins on a controlled native executor. |
| Stop does not yet express the app's shutdown contract. | Native `stop` sets an atomic flag before joining; a concurrent caller can return early. The app's [lifecycle](../prns/native-composition/src/lifecycle.rs) drains LXMF/mailbox work before stopping the node. | All stop waiters observe the same completed shutdown. Service draining remains service/app responsibility and precedes host shutdown. |
| LXMF cannot yet run entirely over canonical host APIs. | [DirectNetwork and LxmfCallbacks](../services/lxmf/src/direct.rs) require destination public keys, authenticated announces, local signing identity, and native event handling. | Supply the narrow missing public capabilities described below; keep signing in Rust and durable work independent of JS. |
| Generic delivery demultiplexing lacks information. | [LinkDelivery](../../prns-host/core/src/events.rs) and [ActiveLinkSnapshot](../../prns-host/core/src/inspection.rs) do not identify the local destination. The current LXMF callback accepts link deliveries without a generic per-destination routing layer. | Expose stable link/destination association and relevant receive metadata before claiming that multiple native services can safely route these deliveries. |
| Mobile ownership still invokes product functions. | [Expo Android service](../prns/platform/android/src/main/java/rs/reticulum/prns/app/expo/PrnsRuntimeService.kt), [runtime](../prns/platform/android/src/main/java/rs/reticulum/prns/app/expo/PrnsAndroidRuntime.kt), and [app storage](../prns/native-composition/src/node.rs). | Move reusable platform mechanics once; supply the app's startup configuration, paths, notification presentation, and services through native composition. |
| Mobile feature support is not established by the desktop host. | [Native host manifest](../../prns-host/impls/native/Cargo.toml) enables a broad desktop transport set; `native_capabilities` is largely static. | Make compiled/mobile transport support explicit and report actual backend capabilities; distinguish supported mechanisms from current permission/radio availability. |

At the audited baseline, the schema contained 19 command cases, 14 unions, 13 records,
33 enums, nine fixed-byte definitions, and ten raw handle definitions. Its
47 explicit raw operations plus 19 projected command operations are an audit
inventory, not a requirement to expose 66 low-level functions to users.

## Ownership and package boundaries

Use these proposed locations to make dependencies reviewable. Final public
package names may change without changing this design.

| Location | Maintained code | Generated code |
| --- | --- | --- |
| `prns-host/core` and canonical schema | Language-neutral meaning, validated constructors, queue policy. Remains free of Expo/UniFFI dependencies and retains `no_std` support. | Existing canonical declarations and schema-derived artifacts. |
| `prns-host/impls/native` | Native session, bounded queues/resources, readiness, command execution and stop completion; platform-neutral embedding hooks. | No parallel implementation of host semantics. |
| `prns-host/bindings/uniffi` | Small foreign-object/async facade and dependency configuration. An `rlib` usable by either native image. | Transport records/unions, conversions, operation conveniences, fingerprints. |
| `prns-react-native` | Expo packaging and platform lifecycle/transport integration, one loader, thin JS ownership/iterator layer. | Host TS/JSI, Swift and Kotlin bindings; canonical JS adapters and exports. |
| Shared repository generator/vendor tooling | One pinned UniFFI/ubrn source, patches, templates, output manifests and checks. | SDK and extension artifacts from their respective authoritative contracts. |
| `applications/services/lxmf` | Existing wire/service/mailbox behavior and one adapter to the public Rust host client. | Optional foreign service API, where consumed. |
| `applications/prns/native-composition` and app | Product composition, contacts, workflows, offline database access, startup/reset policy, and service drain ordering. | Product/service binding transport only; host types imported from the SDK bindings. |

General SDK code and build inputs must not depend on `applications/`. Move the
shared generator/runtime vendoring out of that tree once, with callers supplying
their metadata crate and output manifest. Preserve its existing stale-output,
owned-directory and symlink checks. Do not fork the current four ubrn patches
or maintain a second generator version for the SDK.

Keep the canonical JS value vocabulary in the existing `personal-rns/contract`
entry point, extending its generation where necessary. The [package exports](../../prns-js/package.json)
already separate this entry point from Node and browser implementations. The RN
package must not import the root entry point and accidentally load a browser or
N-API backend. Test Metro and package resolution in a detached consumer. Expo
web should delegate to the existing browser backend for supported host features;
native-only extensions need explicit capability outcomes, not fabricated success.

### Existing app API disposition

Use the current [SDK entry point](../prns/platform/src/index.ts) and
[Rust exports](../prns/native-composition/src/bindings/mod.rs) as the cutover
checklist. These are moves/replacements, not extra permanent forwarding layers.

| Current surface | Destination |
| --- | --- |
| `readDevelopmentNodeSnapshot` | Canonical host inspection comes from the SDK; development/service status remains a small app projection. Remove the app's duplicate host conversion path. |
| `startDevelopmentNode`, `stopDevelopmentNode`, `resetDevelopmentData` | App composition retains startup defaults, service drain ordering and product reset. It delegates node ownership to the shared session. The SDK separately provides general owned-session lifecycle operations. |
| `createGeneratedIdentity`, `createImportedIdentity`, `previewIdentityImport`, `inspectDevelopmentIdentity` | Reuse existing identity primitives; retain app path selection, onboarding state, import presentation and offline inspection policy. Any missing general primitive is proposed independently rather than copying the entire development API. |
| Pairing approval/rejection/initiation, target inspection, remote node changes and Wi-Fi trials | General protocol operations belong in the host projection; app aggregate reads, result presentation and workflow sequencing remain above it. Map individual operations before removing wrappers. |
| Contact create/read/list/delete, aliases, pins, observed-destination saving | App database/product API; no mandatory dependency of the general SDK. |
| LXMF announce, text measurement, send, peers, mailbox list/retry/cancel | Optional LXMF/service API; its native host adapter depends on public Rust host APIs. |
| Ordinary Bluetooth authorization, Android runtime, restoration and foreground ownership | Reusable mechanism moves to Expo integration. App copy, notification presentation and startup configuration remain app inputs. AccessorySetupKit is not part of the app's transport model. |
| `developmentRuntime`, `facade.ts`, `effects.ts` | Relocate remaining product facade/effect integration to the app. Remove the product singleton and product exports from the general SDK. Do not require Effect for basic SDK use. |

## Generate meanings once, adapt mechanics once

Extend [generate-host-contract.py](../../tools/repo/generate-host-contract.py)
and its shared model rather than maintaining another app-specific host schema
walker. Generate UniFFI-compatible Rust mirrors and conversions from the same
semantic definitions used by the other SDKs. Generated transport copies are
acceptable; handwritten field lists and duplicate business validation are not.

The generator needs:

- All closed unions and every case, including configuration and failures;
  the canonical JS `tag`/`data` shape remains public, with UniFFI's `tag`/`inner`
  shape private to generated adapters.
- A small explicit Rust layout/constructor mapping where semantic schema fields
  do not correspond directly to Rust fields. For example, use
  `DestinationName::try_new` and `PrnsLimits::try_new`; do not regenerate their
  validation rules separately. Fail generation on an unmapped type or case.
- Exact `u64`/`i64` values, safe JS integer bounds, optional `safeInt` fields,
  fixed-size byte validation, and `u128`. `DiagnosticsDropped.count` is `u128`;
  pinned UniFFI 0.31.2 has no built-in 128-bit converter. Use a generated
  two-`u64` transport record and convert to canonical JS `bigint` with exact
  range checks. Do not reduce it to a JS number. Swift/Kotlin need an exact
  transport representation too.
- Explicit secret input handling. `IdentitySecret` is a secret 64-byte value,
  not an ordinary debug-printable output record. Construct the existing
  zeroizing Rust owner promptly, avoid generated secret accessors, and clear
  temporary native input buffers. JS callers may retain their own input bytes;
  the SDK cannot promise to erase all JS copies.
- Generated exports, semantic fingerprint, command-to-outcome mappings and
  coverage inventory. Protocol `CommandFailure` remains result data; contract
  mismatch, invalid input, unavailable ownership, and binding failures remain
  distinguishable. Do not collapse them into string exceptions.

Use one canonical command execution path. Typed conveniences such as
`requestPath` are generated over that path if needed. Handwritten code should
scale with lifecycle/stream/resource mechanics, not with the number of commands.
Raw C pointer allocation/free operations become language-owned objects or
iterators; retain an explicit coverage map explaining each such projection.

## Native ownership and asynchronous behavior

The shared session should own `NativeHost` and implement `NativeEventSink` using
the existing [BoundedHostQueue](../../prns-host/core/src/queue.rs). Move resource
storage/stream ownership and readiness out of the C capsule alongside it. Leave
C pointers, C layouts and ABI callbacks in the C adapter. Existing N-API and
cooperative transports can retain transport-specific scheduling; this does not
require rewriting every backend.

Expose a small Rust client that both UniFFI and native services can use for
configuration, commands, snapshots and owned event/resource access. It must not
require services to call through UniFFI, raw pointers, or private node handles.

Foreign futures are driven by the caller; UniFFI does not give arbitrary exported
futures a Tokio runtime. Use a shared nonblocking completion adapter for existing
readiness, register before checking, and unregister before releasing its context.
Provide an async snapshot completion path. Run start/stop/filesystem work on a
controlled native lifecycle executor. Never create one indefinitely blocking
thread per command or stream. See [UniFFI async internals](https://mozilla.github.io/uniffi-rs/next/internals/async-overview.html).

The native owner retains the host until stop completes. Dropping or
garbage-collecting a JS view releases a lease; it must not join the host thread.
A platform-retained session survives loss of its JS views. For a non-retained
session, releasing its last ownership lease schedules the same shutdown on the
native executor; explicit close provides deterministic completion. All concurrent
stop callers wait for the same completion and receive its actual outcome. On a
bounded stop timeout, retain ownership of unfinished work and prohibit
replacement/reset until shutdown truly completes.

The first mobile integration may support one retained session. Make its identity
and configuration explicit, reject conflicting opens, and keep ownership of a
persistent directory exclusive. This is a documented limit of that platform
owner, not a product-specific singleton in the general SDK. A native configured
factory must start/reattach it before JS for background/restoration paths.

Distinguish an owned session from a borrowed client. A standalone SDK caller
that creates a session can stop it. Services and JS attaching to the app's
retained session receive command/query access and release their own leases;
they do not obtain authority to bypass the composed owner's service drain.
The app's stop action and OS stop requests both enter the same native composition
owner, which drains services and then stops the shared host. This keeps lifecycle
policy above the host without making the public SDK a route around that policy.

Do not put long RemoteControl workflows into the current serial command loop:
it awaits each operation, including snapshots. Implement slow general operations
on bounded native task lanes with explicit admission and shutdown accounting.
Cancellation of a JS waiter releases the wait; it does not undo an accepted
network command or committed database mutation. Any actual operation cancellation
must have a separate documented semantic operation.

### Event and resource ownership

Preserve one active consumer per lane. The existing core can release and reclaim
a consumer claim; the C stream destructor already does so. Reattachment does
not need a second queue implementation or a permanent consumer reservation.

Use async **non-consuming readiness** followed by a short, bounded synchronous
drain in the JS iterator. An aborted readiness wait therefore cannot dequeue an
event/resource into a dead promise. Serialize waits/drains per stream; invalidate
the old runtime generation, quiesce active drains, unregister readiness and
release its claim before a replacement runtime can claim it. Apply the same
ownership discipline to resource chunks and uploads, retaining existing bounds.
No synchronous drain may wait, access disk, or join a thread.

Events still queued in Rust retain canonical overflow behavior: application
pressure fails explicitly, diagnostics may drop with an exact count. An event
already returned to JS belongs to that runtime; the SDK does not promise durable
or exactly-once processing across runtime destruction. Services needing durable
processing must consume natively and commit their own records. Do not add a
general replay/acknowledgement protocol to disguise this distinction.

For this app, one native dispatcher owns the raw application lane and routes
events to LXMF using explicit destination association. JS uses the SDK for
commands/snapshots and the service API for mailbox data. A JS attempt to claim
the same raw lane must report that it is already owned. A standalone SDK consumer
without that native dispatcher can own the raw lane itself. Extra observation
streams, if subsequently needed, require their own bounded semantics; they are
not an implicit broadcast promise.

## General host additions needed for app migration

1. **RemoteControl:** project existing opt-in service configuration and typed
   pairing, authorization/grant, target and management operations. Keep combined
   overview queries, prompts, aliases, and user workflows above the SDK. Audit
   persisted authorization location and enabled policy during migration.
2. **Authenticated observation and public identity:** expose accepted announce
   facts with verified identity association, observation metadata and explicit
   bounds, plus destination public-key lookup. Reuse the existing Rust semantics;
   do not substitute lossy `AnnounceHeard` diagnostics for service input.
3. **Delivery attribution:** project the destination/link association required
   to route inbound deliveries and any receive timestamp the service requires.
   Test more than one destination so the LXMF adapter cannot accidentally consume
   another service's traffic. A language-neutral metadata addition is preferable
   to exposing the private routing table or adding an LXMF command to the host.
4. **Platform attachment:** provide a public native embedding hook for prepared
   mobile transports and restoration. Move existing platform machinery out of
   app composition; OS managers and permissions stay in Expo/platform code.

Each addition requires schema, native implementation, other affected projections,
compatibility handling, and conformance on a second applicable target. Adding a
schema variant alone does not implement C/Swift/Kotlin/Node/browser support.
Unsupported cooperative-host capabilities must be explicit. Retain the host
core's `no_std` build and keep UniFFI dependencies in the binding crate.

LXMF's signer stays in Rust. The app may load one native identity source and
supply it to the host and the existing `LocalLxmfIdentity` preparation code,
preserving their association and zeroizing ownership. Do not export the host's
private identity to JS to reconstruct a service. A general signing-capability
API is only needed if this concrete ownership path proves insufficient.

The app's redb/contact/mailbox owner must remain usable while networking is
stopped. Closing a host session must not implicitly close offline product data.
On full shutdown/reset, stop service admission, drain accepted durable work,
stop/join the host, then close/delete app-owned storage as requested. Preserve
existing service restart and concurrent-stop behavior.

## One native image and one binding toolchain

Build the host UniFFI crate as an `rlib`. Supply a default SDK `cdylib` that
contains it. The PRNS app's aggregate `cdylib` depends on that same `rlib` plus
its optional service/product crates. Platform packaging selects **exactly one**
image provider and fails the build if both default and aggregate are included.

Generate SDK and extension namespaces together from the selected image. Configure
external types so an app API uses the SDK's object converter, not another host
wrapper. A single generated loader/image name must cover JS, Swift and Kotlin.
The SDK's public source API is identical for default and aggregate consumers;
image selection belongs in build configuration. Keep generated SDK converters
at a stable import location rather than embedding another copy in app bindings.

Check semantic schema fingerprint, generator/runtime compatibility and each
namespace's UniFFI checksums before native calls. An aggregate image must match
both SDK and app/service contracts. Native rebuild requirements must be reflected
in Expo update compatibility so JS cannot silently use a stale installed image.

This is a statically linked composition model, not runtime plugin discovery.
It follows UniFFI's support for [external types](https://mozilla.github.io/uniffi-rs/next/udl/external_types.html).
Revise the existing C-first [binding guide](../../prns-host/bindings/README.md)
to document this direct-shared-host adapter alongside N-API, preserving the
common semantic and conformance requirements.

## Implementation order and exit criteria

Each stage has an independently reviewable result. App migration waits for the
general host and mobile ownership gates; removal is part of completion.

| Stage | Work | Exit criterion |
| --- | --- | --- |
| 0. Lock contracts and prove packaging | Produce the complete schema/operation coverage map; settle missing envelopes; turn the disposable multi-crate probe into a reproducible fixture. Move shared generator/vendor ownership. | All contract families accounted for; SDK-only and aggregate images produce compatible host namespaces. Strict TS consumer checks and native image checks pass. No application dependency in the SDK build. |
| 1. Extract shared native mechanics | Move queue/lifecycle/readiness/resource ownership from C into the native session. Add shared async completion, nonblocking foreign leases and joined-stop semantics. | C uses the extracted implementation, old duplicate code is removed, and existing C plus native conformance/lifecycle tests pass. Cancellation, stream release/reclaim, pressure and resource ownership are exercised. |
| 2. Generate the general UniFFI facade | Generate full host transport/conversions, typed command conveniences, errors and fingerprints. Bind shared session objects and iterator/resource mechanics. | Every current interface-config case converts; malformed input fails; full scalar/union coverage passes. The persistent two-node TCP journey succeeds through the new API without app/service dependencies. |
| 3. Qualify Expo/mobile ownership | Extract Android/iOS platform glue, configure native startup and image selection, gate transport features, and build an independent Expo sample. | Real Android and iOS builds work. On devices, native startup, JS replacement, reattachment, permission/radio transitions, explicit stop and supported restoration/background behavior pass; one native owner/image is demonstrated. A mobile node can use a desktop conformance peer. |
| 4. Fill general host capabilities | Implement the four focused additions above using public host semantics. | Typed outcomes, bounds, schema/ABI compatibility, another applicable target, and unsupported-backend behavior are covered. Long operations do not block shutdown or unrelated inspection indefinitely. |
| 5. Move LXMF onto the host | Replace its `PrnsNodeHandle` adapter and callback assumptions with the public host client plus the native event dispatcher; retain signing and durable service ownership. | Messaging/discovery with JS absent, two-destination routing, delivery settlement, bounded stop, concurrent stop and restart of retained work pass. Delete the superseded direct-node service adapter. |
| 6. Cut the app over once | Replace app node construction with composition around the shared session. Route canonical operations through the public SDK; retain product/service APIs. | Retained identities, BLE identity, grants, database/mailbox, interfaces, messaging, pairing, stop/restart and reset work. Only one owner may open the old storage. Remove old node owner, snapshot conversion generator, general operation wrappers and duplicate platform glue. |
| 7. Qualify distribution | Package default SDK and aggregate-provider sample for a clean external Expo consumer; exercise native and web exports and release builds. | No workspace path leaks, app-only dependencies, duplicate libraries/runtimes or handwritten host field inventories. Generation is reproducible with stale checks; existing affected SDKs pass regression conformance. Platform limitations are recorded accurately. |

Stages 3 and 4 can proceed independently after their shared prerequisites, but
the app cutover depends on both. Stage 0 is a bounded validation milestone,
not a reopening of the decision to use UniFFI.

### Required acceptance cases

- Integer boundaries: safe-int min/max and rejection outside range; `u64` at
  `0`, `2^53-1`, `2^53`, and maximum; `u128` across the 64-bit boundary and at
  maximum. Every union variant, optional present/absent, fixed-byte wrong length,
  and private-constructor validation is covered without weakening TS strictness.
- Cancellation races: before readiness, after readiness but before drain, during
  command settlement, and runtime invalidation with resource/upload work. No
  callback into an invalid JS runtime, orphaned claim, unbounded buffer or host
  thread join on JS destruction.
- Ownership: two claim attempts, close/reclaim, concurrent stops, stop timeout,
  repeated start/stop, configuration conflict, and default/aggregate native image
  collision. Native service event ownership must be observable to SDK consumers.
- Persistence: retain current path layout during initial cutover. Verify the
  same PRNS and BLE identities, LXMF signer/destination, pairing authorization
  and mailbox/contact data. No silent identity regeneration. If a format/path
  change is actually required, ship its migration and recovery strategy separately.
- Shared behavior: C and native host regressions after extraction, the common
  [interface fixtures](../../prns-host/conformance/interface-configs-v1.json), and
  [persistent two-node journey](../../prns-host/conformance/persistent-two-node-v1.json).
  Device lifecycle checks are additional evidence, not replaced by desktop tests.

## What handwritten code remains

The realistic target is no duplicated handwritten **host meanings**. There will
still be one generator implementation, one foreign-object/async adapter, platform
lifecycle/transport code, one LXMF host adapter and app composition. Those have
different responsibilities and cannot all be generated from record definitions.
The current code should move into those owners wherever it already performs the
job; migration is incomplete while both implementations remain active.

In particular, do not duplicate 19 command methods, schema field lists, snapshot
hydration, queue admission policy, resource ownership, private-key loading policy,
or platform supervisors across C/UniFFI/app packages. Generated Swift/Kotlin used
internally by Expo are an implementation detail, not a replacement for the
existing public C-backed Swift/Kotlin SDKs.

## Historical evidence collected for this plan

The source audit validated the canonical schema and inspected native/C queue,
command, resource and lifecycle ownership; full app/native tests were not rerun
for documentation changes.

A disposable two-crate workspace used pinned UniFFI 0.31.2 and the repository's
locked ubrn generator. The SDK crate exported an `Arc` object, an async mutation,
and a `{ high: u64, low: u64 }` record. An aggregate app library accepted and
returned that SDK object. Results:

- The aggregate library compiled offline on macOS.
- JSI2 TypeScript, Swift, Kotlin and Python bindings generated from the same
  library. Both JSI namespaces selected `prns_image_probe`; the extension
  imported the SDK's object converter.
- Generated TS plus a consumer passed strict type checking with
  `exactOptionalPropertyTypes`; the temporary generated files had their
  `@ts-nocheck` lines removed for this check. A number supplied to a `bigint`
  argument was rejected as expected.
- Both generated Swift namespaces passed a combined macOS Swift type check
  against their generated C module maps.
- Generated Python bindings executed cross-crate object sharing and async
  mutation against that library. Exact 64-bit values and two-limb values through
  `2^128-1` round-tripped.

This original probe proved a metadata/linking/conversion slice. It did **not**
qualify the JSI runtime, actual `NativeHost`, Hermes reloads, iOS/Android linkage,
native restoration, custom-image package substitution or C-session extraction.
Later implementation and qualification are recorded in the
[implementation record](react-native-sdk-implementation.md#current-qualification);
the probe itself is not device evidence.

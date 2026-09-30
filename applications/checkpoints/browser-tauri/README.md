# Browser and Tauri architecture checkpoint

This is a dated architecture decision, not current platform status or a browser
or desktop provider. It
retains no provider package, storage adapter, Tauri shell, or disposable spike
code, and this re-review adds no provider or runtime implementation.

The source observations below belong to the recorded baseline. Android has
since gained a native provider and generated bindings; web and Tauri still have
no application runtime provider. See the [current roadmap](../../docs/roadmap.md)
and [validation limits](../../docs/validation.md). Re-audit the proposed upstream
seam before implementation.

## Audited baseline

- Original Prns baseline: `68ee3156d268152d77f5e5a2ece3578ba6473d4e`.
- Prior re-review baseline: `5126c94fc21e0c5fb20f2478a1621c7ef5cce1a9`
  (`Derive default interface gravity from bitrate`).
- Latest baseline covered by this checkpoint:
  `81e1eeb00080c44ef55b440f44d7bac13e3ee044` (`upstream/trunk`, `Gate
  resource offload by runtime capability`).
- Between the prior and current re-review baselines, the browser runtime folded
  its separate protocol-crypto and Resource offload calls into one typed
  `BrowserWork` continuation boundary. `RuntimeHost` now selects `Inline` or
  `BrowserWorkers`, drains `AnnounceVerify`, `LinkProofVerify`, `ResourceSeal`,
  and `WholeResourceOpen` work, and lands each completion through the runtime.
- The same range adds bounded Tokio scheduler policy, prioritizes interactive
  egress over bulk Resource work, and capability-gates Resource offload. These
  changes belong inside the Prns runtime; they do not add an application
  message lane or transfer node scheduling policy to a future provider.
- The five-lane DedicatedWorker wire, client, protocol, and engine-bridge files,
  together with the Tokio shared-instance sources, remained byte-identical in
  the new range. Worker ownership, shared-instance election, and the direct
  Tauri composition below therefore remain unchanged.

## Browser decision

Keep a `DedicatedWorker` as the first browser owner. Nothing in this checkpoint
justifies a `SharedWorker`, leader election, or browser storage design. The
existing owner already creates distinct control, event, page-capability,
projection, and shutdown `MessagePort`s and transfers all five to exactly one
engine worker.

The useful mechanics are real:

| Lane | Existing behavior | Aggregate consequence |
| --- | --- | --- |
| Control | Batches calls, limits queued commands and control calls, caps a batch at 1 MiB, correlates settlements, and reports busy admission | The generic batch sender/receiver can carry generated application calls |
| Events | Retains application-event count and bytes, requires one-in-flight acknowledgement, and reports fatal application backpressure; diagnostics are bounded and emit a dropped gap | Preserve these semantics, but the sender currently accepts only base Prns events |
| Page capabilities | Uses a separately bounded request/settlement channel for page-only Bluetooth, USB, and network work | Preserve the lane; generated aggregate capability cases cannot enter its current closed union |
| Projections | Coalesces the latest update per projection tag, acknowledges one in-flight batch, and bounds pending synchronizations | Preserve coalescing and recovery; application projection tags cannot enter its current closed union |
| Shutdown | Uses a dedicated, one-shot `Stop` port and returns stop, persistence, snapshot, and Host-snapshot state before worker termination | The lane is isolated, but its page-side wait has no explicit deadline; a stuck interface close or persistence save can therefore leave `stop()` pending indefinitely |

The typed `BrowserWork` queue is an internal WASM/runtime execution seam, not a
replacement for these five page/worker lanes. It authorizes and correlates
bounded protocol and Resource continuations while the existing control, event,
capability, projection, and shutdown ports still own browser-process
orchestration. A future application aggregate should reuse both layers: it
must not copy the continuation scheduler into TypeScript or treat
`BrowserWork` as an application protocol.

A disposable compile/runtime spike used the existing generic
`BatchedPortSender` and `BatchedPortReceiver` with generated-like
`DescribeManagedTarget` and `ListContacts` calls. It proved that the transport
primitive accepts an application-owned algebra and returns `Busy` at its queue
bound. Type assertions also proved that the current `WorkerCall`, capability,
event, and projection surfaces reject those application cases. The spike was
removed after the result was recorded.

Direct reuse of the complete client is not yet possible:

- `createDedicatedWorkerPrns` always constructs `./worker.js`;
- `DedicatedWorkerPrns` is private;
- worker startup is a private `startEngine` that always calls `Prns.create`;
- the control, event, capability, projection, and shutdown unions are base-Host
  unions; and
- the package exports no worker-substrate entrypoint. `worker_engine_bridge`
  binds base-engine sinks and dispatchers, not an engine factory or an
  application protocol family.

### Smallest upstream seam

Before a browser aggregate is implemented, extract the current page/worker
orchestrator as one public, backend-neutral DedicatedWorker substrate. It must:

1. accept an explicit statically bundled worker entry rather than hard-coding
   `./worker.js`;
2. let the worker entry supply its compile-time engine factory;
3. parameterize the five existing lanes with generated protocol families while
   retaining the current batching, admission, acknowledgement, projection-gap,
   startup, persistence-flush, and termination rules;
4. add a finite shutdown deadline with forced worker termination and a typed
   incomplete-shutdown result; and
5. leave base Host messages owned by Prns and application-service messages
   owned by the application composition.

This should be an independently tested base-Prns branch. Exporting only the
low-level batch classes would make the application duplicate startup,
correlation, backpressure, projection, and shutdown control, which is the
parallel control plane this checkpoint is meant to prevent. A worker URL or
engine factory alone is also insufficient because the public client has no
application operation, event, or projection path.

The extracted orchestrator should delegate protocol and Resource work to the
existing `RuntimeHost`/`BrowserWork` boundary. It must preserve the runtime's
capability gate, continuation correlation, stale/collision handling, and
interactive-over-bulk scheduling rather than introducing a second work queue
or scheduler in the application layer.

## Direct Tauri composition

The first desktop provider should link the application aggregate directly. It
does not need a reusable Tauri plugin.

The process-owned start sequence is:

1. Acquire the Tauri product-process singleton. A second launch delegates to
   the first process. This is distinct from Prns network-instance election.
2. Open one application `Installation` for the process. Load or create a
   persistent shared-instance transport identity under that installation. It
   is not the primary app identity, Bluetooth identity, or either RemoteControl
   identity.
3. Build `SharedInstanceCredentials::from_identity_secret` from that transport
   identity. For the local-owner configuration, use its derived RPC key. For an
   explicitly configured external owner, override it with that owner's
   compatible RPC key; never assume the locally derived key authenticates to a
   different owner.
4. Construct the Prns node with `ManuallyAttached` interfaces and obtain its
   handle. Do not construct a physical interface yet.
5. Call `join_shared_instance(&handle, intent).await`.
6. On `BecameInstance`, construct and attach the configured physical
   interfaces, then run the node. The app may report its own configured/local
   interface inventory.
7. On `JoinedAsClient { of }`, run with only the shared-bus client installed by
   `join_shared_instance`. Report `of` as the owner endpoint. Do not construct
   physical interfaces and expose owner inventory/status/configuration as
   `Unavailable { reason: "remoteOwnerControlNotProjected" }`.
8. Store the aggregate in Tauri application state. Windows acquire bounded
   subscriptions; closing a window releases subscriptions, not the Installation
   or Host. Explicit process shutdown owns reverse-order stop.

The current public Prns API supports this ordering. Its election binds both bus
and control endpoints before returning `BecameInstance`, retries joining after
a bind race, and returns `JoinedAsClient { of }` after installing only the
shared-bus interface. Existing integration tests exercise both roles, refusal,
partial-bind cleanup, and traffic across the joined bus.

Although `SharedInstanceRpcClient` has owner-query operations, this checkpoint
does not expose them. Authentication, a bounded app contract, permissions, and
an explicit remote-control projection must be designed together before remote
owner inventory or mutation becomes available.

## Static provider selection at the audited baseline

Application code keeps importing the single stable
`@/native/runtime-provider` specifier.

- Metro selects `runtime-provider.ios.ts` for iOS; that file alone imports the
  Expo native owner.
- Metro selected `runtime-provider.android.ts` for Android; it was a typed
  unavailable provider at this baseline. It now selects the native Android owner.
- Web selects `runtime-provider.web.ts`; the ordinary TypeScript fallback also
  explicitly re-exports this web provider. The audited web export contained the
  web unavailable case and no `PrnsApp` native-module reference.
- A future Tauri build must alias that exact specifier to a dedicated
  `runtime-provider.tauri.ts` at build time. The build must fail if the alias or
  entry is absent. It must not select Tauri by inspecting `window`, globals, or
  user-agent state at runtime, and the webview must never start the WASM owner.

These static entries prevent one bundle from selecting two owners. The Tauri
entry and alias remain deferred until the desktop slice exists.

## Recorded re-review evidence

- `npm --prefix prns-js test` passed 111 tests against the current integration
  source, including the unified browser-work boundary, Resource continuations,
  provider-free contract consumer, and the unchanged worker lanes.
- A separate full `npm --prefix prns-js run build:code` and `npm pack` passed
  while producing the artifact used by the compatibility refresh.
- `cargo test --locked --manifest-path validation/integration/Cargo.toml --test local_instance`
  passed five shared-instance role and traffic tests.
- No browser provider, Tauri shell, or platform qualification was run for this
  re-review.

Browser storage, multi-tab ownership, a browser provider, Tauri packaging,
command permissions, window/tray/sleep journeys, signing, updater behavior,
and broader platform qualification remain deferred. Android implementation
proceeded later and is not a remaining item from this checkpoint.

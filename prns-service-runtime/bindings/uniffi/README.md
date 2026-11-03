# Service runtime bindings

This independent image constructs `prns-service-runtime::groups::standalone::Owner` from a private catalog path, the canonical host configuration, and explicit resource options. It returns the same identity, group, and host clients used by Rust consumers. This crate owns only construction and cleanup wrappers; it declares no parallel domain methods or record models.

## Consumer lifecycle

1. Choose `createNew` for a new private store or `openExisting` for an existing
   one; use the offline variants when no host is needed yet.
2. Await `ready()`, obtain the canonical identity/group clients, and configure or
   recover their services through those clients.
3. Await `networkReady()` before borrowing a host client. For an offline owner,
   first call `startNetworking(host)` with the desired host configuration.
4. Await `stopNetworking().wait()` when only networking should stop. Local
   history and identity clients remain available.
5. Await `shutdown().wait()` when the entire owner should close. Keep the returned
   close handle if that wait is interrupted so cleanup can still be observed.

## Readiness and ownership

`ServiceRuntime.createNew` and `openExisting` return once the service runtime owner exists and request networking. Await `ready()` before obtaining local identity/group clients, and `networkReady()` before borrowing the host or connection binding. `createNewOffline` and `openExistingOffline` require no host configuration and prepare only local data. `status()` distinguishes local startup, ready, closing, failed, unresolved cleanup, and closed state; `networkStatus()` independently reports the network phase, generation and failure. `latestIdentityDiagnostic` projects the identity owner's existing retained error and optional identity reference. It is historical evidence, independent of the current phase or cleanup outcome; native Rust callers can retain the same rich diagnostic observer. Dropping a readiness future does not cancel startup. Reopen validates existing selected schemas. `ServiceRuntimeOptions.startupRecovery` explicitly selects `.dormant` or `.recoverCurrent`; the latter schedules current saved Enabled/pending work during network startup before network readiness, without beginning inert reservations. Network readiness acknowledges that admission pass, not completed identity activation; canonical identity snapshots and retained diagnostics expose progress and failures.

`startNetworking(host)` attaches a fresh host generation to the existing local owner. `stopNetworking()` fences the captured generation and returns a retained `ServiceRuntimeNetworkChange`; await its `wait()` to observe actual network cleanup. The same local clients remain readable while offline or after network startup failure. Full `shutdown()` closes the installation's local owner as well.

The concrete profile supports four catalog identities, 32 retained identity operations, four service declarations per identity, and 16 identity-runtime leases. Callers select queue, cache, carrier, catalog, retention, and optional archive budgets explicitly; all bounds are checked before host or filesystem effects. A different Rust profile can use the underlying generic constructors with its own concrete owner.

The owner retains one service executor (one Tokio worker) and the host's separate executor. `shutdown()` synchronously fences local admission and new mutations through every borrowed host client. Host reads and previously accepted operations remain available during local drainage. Accepted identity/group work and service cleanup drain first; the host then stops and joins; only then is the service executor released and the retained close result published. `ServiceRuntimeClose.status()` remains inspectable after the foreign owner is released. Unproved drainage remains owned with a pending close and an explicit unresolved status. Generated Kotlin `AutoCloseable.close()` disposes the foreign reference; `shutdown()` is the explicit operation that returns a drain observer.

The Rust `create_new_with_embedding` / `open_existing_with_embedding` seam retains platform interface preparation and combines the supplied callback with the group router before host startup. The foreign default uses the ordinary host config and needs no ambient Tokio executor, foreign runtime handle, or Rust-provided key.

Host configuration uses checked custom values whose lift error is the existing `BindingError`. Constructors preserve that exact error type: invalid resource selection is `InvalidInput`, and a failure to establish the lifecycle worker is `Backend` with diagnostic context. Once the owner exists, `ready`, `status`, and `shutdown` preserve the typed `ServiceRuntimeError`, including uncertain cleanup. Domain operations retain their own canonical typed errors.

The `archive`, `text`, `contact-card`, and `connections` features are independent, default-off selections. The contact-card codec's canonical verification namespace is included only when selected. Nothing depends on React Native, an application, or LXMF.

With `connections`, `createNewWithConnections` and `openExistingWithConnections` accept an externally owned `ConnectionProfiles`. The composed owner fences its binding immediately, drains identities/groups, retires the binding, and then stops the host. `connectionBinding()` returns the same borrowed canonical connection client; it cannot close the binding or the profile store. The profile owner remains editable across composed Stop/open and has a separate lifetime. Construction retains the existing checked `BindingError` boundary for nested host configuration; profile methods preserve `ConnectionError` and exact conflict snapshots.

Failed service attachment retains a typed `ServiceAttachmentFailure` and bounded
diagnostic detail in `ServiceRuntimeError.Attachment`. Group causes use the
canonical `GroupApiError`; identity and physical-store causes retain their own
classifications. `ready()` and `status().failure` expose this cause even when
startup fails before an identity diagnostic observer exists. It is not a retry
instruction or permission to reset saved data.

## Verification

On macOS with Xcode command-line tools:

```sh
python3 validation/run.py run --suite native-service-bindings-swift
python3 validation/run.py run --suite native-connections-bindings-swift
```

The checker uses `tools/uniffi/tooling.py` to build one image and generate Swift from the owning namespaces. It checks the minimal and selected shipping dependency graphs and construction metadata, then compiles and executes a plain Swift consumer. The consumer creates its own named identity, signs and verifies a public claim, explicitly adds and enables groups, uses the ordinary retained group client, closes, and reopens local history. It also checks malformed host secrets, resource budgets, and identity references through actual generated lifting.

The Rust adapter and selected feature checks are registered separately:

```sh
python3 validation/run.py run --suite native-service-bindings --suite native-service-bindings-selected --suite native-service-bindings-clippy
```

The runtime's [owner checks](../../README.md#owner-checks) exercise startup
cancellation, observer bounds and callback panics, blocked startup closure,
finalization, and Rust consumption without an ambient runtime. Foreign checks
exercise the generated boundary; neither substitutes for mobile platform tests.

The optional connection fixture exercises strict create/open, checked IDs/configuration/budgets, stopped CAS editing, independent `HostSession` binding, two-phase cleanup, composed host generations, and durable profile deletion. Both fixtures share only their explicit test budgets and host configuration.

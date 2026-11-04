# UniFFI host transport

This crate is an `rlib` containing generated value transport and a thin foreign ownership facade. The standalone `image/` crate supplies the default native library. An application may link this same crate into its own aggregate image and return `HostClientHandle::borrow` from its extension namespace. Only one selected image supplies the host and app namespaces in a process.

`HostSession` owns the shared native session. `HostClientHandle` is a borrowed command/query lease and has no stop method. Application and diagnostic streams retain the shared queue's sole-consumer claims. Readiness futures can be cancelled without consuming an event; accepted native protocol tasks survive a dropped foreign waiter. Native shutdown cancels and joins the bounded task lane before ownership is released.

For `HostSession.stop`, `BindingError.Backend` means shutdown joined but failed;
foreign owners can release their handles while preserving the failure for every
waiter. `BindingError.OwnershipUnavailable` does not establish joined shutdown,
so callers retain ownership and may retry. Unknown transport errors also do not
establish shutdown. Snapshot errors preserve the generated `Busy` and `Stopped`
variants instead of becoming generic backend failures.

## Generation

Run the owning tasks from the repository root:

```sh
./tools/prns run repo.host-contract.generate
./tools/prns run repo.host-contract.check
```

The check verifies every checked-in source projection. Edit canonical declarations
or generator rules rather than their generated outputs.

- `src/transport.generated.rs` and `typescript/host-adapter.generated.ts` project the canonical host JSON schema, including session configuration, commands, outcomes, snapshots, lifecycle, errors and events.
- `src/remote_control.generated.rs`, `typescript/remote-control-adapter.generated.ts` and `schema/remote-control-v1.generated.json` are derived from existing public Rust Remote Control declarations and traits. The generator follows nested types, instantiates generic error types and emits the forwarding methods. There is no application operation or field inventory to synchronize.
- Private bounded storage uses explicit constructor/accessor mappings in `tools/repo/host_contract/remote_control.py`. These mappings call existing Rust validators. Unsupported declaration syntax and newly unmapped private wrappers fail generation or Rust compilation.
- The selected native library is the source for UniFFI's platform metadata. Do not set a fixed library name in per-namespace configuration; the same namespace can be supplied by the default or aggregate image.

The [native extension inventory](../../native-extensions.generated.md) also feeds
`coverage.generated.json`; generation validates its public source references and
complete target availability. Destination public-key lookup is a native Rust and
UniFFI extension. Authenticated announce observations and prepared attachment
hooks are native Rust embedding APIs only. They are not foreign event feeds, and
broader foreign observation parity remains unimplemented. This accounting lives
outside the portable wire schema and does not alter existing fingerprints.

Host schema version 2 adds destination attribution and arrival time to link delivery. The physical C ABI remains version 1; existing schema negotiation rejects an older semantic contract. Host and Remote Control semantic fingerprints are separately exposed by `binding_contract()` and checked by the React Native provider.

## Values and secrets

Full-width `u64` values remain exact; TypeScript uses `bigint`. A `u128` uses two `u64` limbs. JS safe-number fields are checked before native conversion. Identity material, Wi-Fi passwords and pairing invitation input codes are lifted into zeroizing Rust owners. Secret-bearing configuration is input-only, and native transport records do not derive `Debug` or `Clone` for it. Pairing invitation codes are returned only where the existing pairing ceremony intentionally publishes them for user confirmation.

Remote Control config is opt-in and takes caller-selected identity sources, grants, capabilities and self-announcement policy. Generated pairing and access operations use the common bounded native task lane. Its typed observations share the application queue and consumer claim. C, N-API and cooperative/Wasm projections explicitly do not support this native Remote Control extension yet; their existing host APIs remain available.

The alloc-only helpers declared by `prns-core::remote_control` provide the
canonical public controller-card and recipient-bound target-access-card text
codecs and six-digit transcript comparison-code formatting. Generated foreign
functions preserve their owning error inventory. Decoding a card neither grants
access nor verifies its source; consumers must review provenance independently.
`save_remote_control_target_access_card` then checks the full local controller
identity, refuses conflicting saved access, and durably publishes before
activation. The separate explicit `save_remote_control_target_access` operation
can replace reviewed access. Both use the existing persistence lane; cancellation
of a waiter does not roll back an admitted save, and `NodeStopped` may mean an
unknown acknowledgement outcome. The older `set_remote_control_target_access`
remains an explicitly volatile capability. No storage-capable implementation is
assumed for embedded or cooperative runtimes.

The native adapter explicitly selects the core `lora-2g4` vocabulary so its generated radio declarations include both canonical band variants. This says nothing about attached hardware support. Portable/default core consumers retain their own feature selection. `remote_control_radio_profile` and `remote_control_ghz24_profile` delegate the existing checked constructors and return their existing typed validation failures. `RemoteControlBandProfile` is the foreign name of core `LoRaProfile`; the separate existing `RemoteControlLoRaProfile` remains its bounded wire representation. Conversion helpers use the owning codec, so applications never implement its compact grammar. `remote_control_request_requires_administrator` delegates the protocol's request policy. Private radio record lifting derives constructor parameter order from the owning source and rejects malformed values before a request can be admitted.

## Optional saved connections

The default-off `connections` feature exposes the native profile owner without an application, identity catalog, or group service. `ConnectionProfiles.createNew(path, profiles, options)` explicitly selects the initial profiles and bounded store capacity; `openExisting` is strict and never inserts defaults. Its ordinary `ConnectionProfileClient` reads snapshots and replaces profiles with the exact incarnation/revision guard. Saves remain available while the host is stopped; a committed save and the binding's `appliedRevision` are separate facts.

`ConnectionProfiles.bindHost` returns a `ConnectionHostBinding` that owns only its claimed interfaces on the supplied `HostSession`. `client()` returns a borrowed status/snapshot/retry client with no cleanup authority. `fence()` blocks new reconciliation while retaining current attachments for protocol drainage. `shutdown()` then requests retained retirement; its observer's `wait()` never requests a retry. An explicit later shutdown can retry a failed retirement, and all observers see the current cleanup state while `cleanupFailure` preserves the latest historical failure. Only a completed close establishes resource release. The profile store's separate `shutdown()` fences saves and retains its bounded worker cleanup. Borrowed clients neither own the store nor keep a host binding active.

`src/connections.generated.rs` and `schema/connections-v1.generated.json` project the actual declarations in `prns-host-runtime::connections` and the host attachment error. Client methods and TCP endpoint helpers are discovered from their owning implementations; new unsupported fields fail generation. Profiles reuse the existing canonical interface/routing/bitrate transport. Checked profile IDs and platform-sized budgets are lifted before effects. `ConnectionError` preserves the native variant inventory and includes the exact current snapshot on CAS conflict. Native database/file/thread causes are output-only diagnostic strings; the native Rust API keeps its rich causes. No raw attachment-scope authority is exposed.

## Verification

`./tools/prns run repo.host-uniffi.check` builds the real default image, checks generated sources, runs conversion tests, generates Python bindings and exercises native foreign conformance. The baseline journey covers cancellation/reclaim, persistent two-node requests, streamed resources, concurrent stop, and restoration. This is desktop foreign/native evidence; mobile builds and Hermes runtime evidence are separate qualification gates.

On macOS, `python3 validation/run.py run --suite native-host-swift` uses the
shared native-only binding recipe, compiles the generated Swift module, links it
to the real host image, and runs an ordinary Swift consumer. It checks canonical
enum marshalling (including exact `UInt64.max` transport), the native contract
fingerprint, typed native input rejection, and owner/borrowed-client shutdown.
The maximum integer transport case does not relax checked native field limits.
This gate needs Xcode command-line tools and no React Native generator or npm
installation. Set `CARGO_TARGET_DIR` to reuse an existing native build directory.

`python3 validation/run.py run --suite native-host-connections-swift` checks the default and selected host shipping closures, then executes the actual independent host/profile journey: stopped CAS edits, malformed input rejection before effects, retained interface/store shutdown, and strict reopen without reinserting defaults. It uses only the standalone host image, with no aggregate, group, identity-catalog, app or React Native dependency. The default checker does not enable connections.

`python3 validation/run.py run --suite native-connections-bindings-swift` reuses that same host-owned journey and closure assertion, and additionally checks composed owner creation, fencing, generation changes and restart. The composed test remains a separate consumer of the canonical host API.

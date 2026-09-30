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

Run `python3 tools/repo/generate-host-contract.py` at the repository root. Its `--check` mode verifies every checked-in source projection.

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

## Verification

`python3 tools/repo/check-host-uniffi.py` builds the real default image, checks generated sources, runs conversion tests, generates Python bindings and exercises native foreign conformance. The baseline journey covers cancellation/reclaim, persistent two-node requests, streamed resources, concurrent stop, and restoration. This is desktop foreign/native evidence; mobile builds and Hermes runtime evidence are separate qualification gates.

# Generated-binding pilot

Enable `uniffi-bindings` on the existing `prns-app-native` crate. It adds no node
constructor or runtime: `read_snapshot()` submits to the existing supervisor and
returns the existing full `DevelopmentNodeSnapshot`. `binding_contract()` returns
both app and canonical Host contract identifiers. Native startup's generation ID
and primary identity can be compared directly with the generated snapshot.

`U64String` now stores a `u64`. Its legacy JSON/ts-rs contract still uses canonical
decimal strings; UniFFI exposes the same value as `bigint` / `UInt64` / `ULong`.
The C/JSON bridge remains temporarily while platform/SDK consumers migrate.
`export_contract --fingerprints` emits the semantic app/Host identifiers as JSON
without generating a second TypeScript model.

## Canonical HostSnapshot

Run `python3 applications/tools/generated-bindings/host_contract.py` from the repository root.
The generator locates the app's locked `prns-host` dependency using Cargo metadata
and reads its canonical `../schema/host-contract-v1.json` HostSnapshot closure and emits:

- `src/bindings/host_generated.rs`: transport records and conversions over the
  actual `prns_host` records, validated fixed bytes and canonical enum names.
- `uniffi.toml`: checked safe-integer and branded-byte TypeScript conversions,
  plus Swift/Kotlin module naming.
- `bindings/typescript/host-adapter.generated.ts`: conversion to the existing
  `personal-rns/contract` types, including omitted optional properties.

The generator owns these serialization artifacts. Runtime state and inspection
remain canonical `prns_host::HostSnapshot`, and the no_std core does not depend on
UniFFI. `--check` rejects stale outputs. `safeUint` stays a checked JS number;
exact `u64` counters stay bigint. The small adapters are generated, not another
handwritten domain model.

## Generate from one native library

Keep Cargo targets on the external drive. Example environment for this pilot:

```sh
export CARGO_TARGET_DIR=/Volumes/wavlink/dev/prns-jsi2/app-target
```

Build the host library from the repository root:

```sh
cargo build --locked --manifest-path applications/Cargo.toml \
  -p prns-app-native --features uniffi-bindings,host-test --lib
```

For iOS/Android builds, use `apple`/`android` instead of `host-test`. Swift/Kotlin
platform lifecycle owners and jsi2 MUST use the same built native image; do not
link a static copy beside the dynamic image loaded by JavaScript.

Generate jsi2 from `applications/prns/native-composition`, using the pinned,
strict-TypeScript-fixed generator executable in `UBRN_BIN`:

```sh
"$UBRN_BIN" generate jsi2 bindings --library "$CARGO_TARGET_DIR/debug/libprns_app.dylib" \
  --ts-dir bindings/typescript --lib-name prns_app --no-format
node bindings/normalize-generated.mjs
```

The generated `prns_app.ts` and `prns_app-ffi.ts` must stay adjacent to
`host-adapter.generated.ts`. Their async functions accept `{signal}` for caller cancellation. Module initialization and its UniFFI checksums follow
jsi2's generated entry point. The app/Host fingerprints retain their separate
semantic compatibility role.

Build the development-only native generator with feature `uniffi-bindgen`, then
run it with the explicit config (unlike the jsi2 CLI's metadata discovery):

```sh
cargo build --locked --manifest-path applications/Cargo.toml \
  -p prns-app-native --features uniffi-bindgen,host-test --bin uniffi-bindgen
"$CARGO_TARGET_DIR/debug/uniffi-bindgen" generate \
  --library "$CARGO_TARGET_DIR/debug/libprns_app.dylib" \
  --config applications/prns/native-composition/uniffi.toml \
  --language swift --language kotlin --no-format --out-dir "$NATIVE_BINDINGS_OUTPUT"
```

Swift module name is `PrnsAppBindings`; Kotlin package is
`rs.reticulum.prns.app.bindings`. Kotlin's library name is `prns_app`.

## Validation

```sh
python3 applications/tools/generated-bindings/host_contract.py --check
cargo test --locked --manifest-path applications/Cargo.toml -p prns-app-native \
  --features uniffi-bindings,host-test --lib --test generated_snapshot
cargo clippy --locked --manifest-path applications/Cargo.toml -p prns-app-native \
  --features uniffi-bindings,host-test --lib --tests -- -D warnings
node --test applications/prns/native-composition/bindings/tests/host_adapter.test.mjs
```

The focused tests cover full UniFFI binary snapshot round-trip, native-before-
foreign-executor ownership, exact integers, canonical HostSnapshot conversion,
async yielding, cancellation without stopping the node, lifecycle lock
contention, and a stale timeout that must not mutate a replacement generation.
They do not substitute for actual iOS/Android reload and lifecycle integration.

The existing platform admission remains required: iOS accessory/protected-data
startup, Android foreground-service/Bluetooth ownership, and Android outbound
preflight. Snapshot reads do not drive radio recovery.


## Native bootstrap and remaining API

Native lifecycle owners call synchronous `native*` functions only from their
existing background/lifecycle queues. These may perform blocking I/O or wait for
Start/Stop. They must not be called directly on Hermes:

- `nativePrepareStorage(storageRoot)` initializes the existing supervisor-owned
  database and returns `NativeStoragePreparationOutcome`: `Prepared`,
  `Unavailable { detail }`, or `DevelopmentResetRequired { reason }`.
- `nativeInspectIdentity`, `nativeCreateGeneratedIdentity`, and
  `nativeCreateImportedIdentity` use that platform's protected storage path.
- `nativeStart`, `nativeStartWithAppleBluetoothCentralRestoration`,
  `nativePrepareAppleBluetoothCentralRestoration`, `nativeStop`, and `nativeReset`
  preserve the existing process-owned supervisor and platform admission order.

`previewIdentityImport` is a bounded synchronous parse requiring exactly64 bytes.
The nineteen domain operations use async exports: pairing initiate/approve/reject,
Describe/AnnounceSelf, seven contact operations, and seven LXMF operations.
All storage operations use the owner established by native bootstrap; JavaScript
cannot supply paths or silently open/reopen the database. Reset requires another
native bootstrap before offline database access. Start already establishes the
same owner as part of its existing composition.

Async admission uses `try_lock`, bounded existing lanes, and oneshot responses.
No asynchronous entry performs path resolution, database opening, synchronous
reply waiting, or completed-worker joining. A process-wide `futures-timer` timer
driver preserves query deadlines when no Tokio node runtime exists; it does not
execute commands or own a node. Describe caller drop retires its network future
and admission. Accepted contact/mailbox mutations and sends remain owned and
stop-drained after caller cancellation; no synthetic write timeout invites a
second insert while the first may still commit.

UniFFI uses explicit optional-value tags: generated `undefined` is the semantic
`None`, replacing the old JSON-only distinction between a missing field and
`null`. Fixed16/32-byte inputs validate in custom lifts; u64 values stay exact.
Contact normalization, pin/identity rules, mailbox pagination, pairing input and
text semantics remain shared Rust checks. Native input/path/restoration size
bounds are shared with the legacy ABI. Swift/Kotlin/JS generated codecs may carry
native lifecycle outcomes through the small Expo admission wrappers without
handwritten field mappings or a JSON compatibility model.

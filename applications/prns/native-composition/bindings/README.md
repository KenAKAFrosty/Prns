# Generated application bindings

`@prns-internal/native-bindings` is the generated interface to the existing
`prns-app-native` composition. UniFFI is enabled by default. Swift/Kotlin
lifecycle owners and generated JavaScript calls use one `prns_app` shared native
image per target, with the same supervisor, storage owner, generation and node
identity. Native startup and iOS restoration can precede JavaScript. Loading or
reloading JavaScript does not construct another node or Tokio runtime.

## Generate and build

From the repository root:

```sh
npm --prefix applications run api:generate
npm --prefix applications run api:check
npm --prefix applications run bindings:ios -- --sim-only --targets aarch64-apple-ios-sim
npm --prefix applications run bindings:android -- --targets arm64-v8a --release
```

Generation refreshes TypeScript, Swift, Kotlin, semantic contract fingerprints
and the canonical HostSnapshot adapters from the same Rust API. Checking rejects
stale output without rewriting it. The mobile commands build the shared image
owned by this package; the normal native client build helpers invoke them before
Expo prebuild and autolinking. The Expo module must not link or package a second
copy of `prns_app`.

See the [generation guide](../../../tools/generated-bindings/README.md) for
toolchain requirements, target selection and disposable build caches. Generated
files are outputs of that workflow and must not be edited directly.

## Imports and platform admission

Use `@prns-internal/native-bindings` for types, enum factories, generated
converters and fingerprint constants. This entry is safe to import on web and
does not install the native runtime. The `/native` entry installs the JSI runtime
and initializes the generated UniFFI ABI checks. The Expo SDK loads it lazily
after checking native capability, then checks the app and canonical Host semantic
fingerprints separately. Application code should use that SDK so platform
admission accompanies execution.

The execution boundary is:

- `readSnapshot` and nineteen domain operations call generated async bindings:
  pairing initiate/approve/reject, Describe/AnnounceSelf, seven contact operations
  and seven LXMF operations. `previewIdentityImport` is a bounded synchronous
  parse of exactly 64 bytes; `bindingContract` reads immutable identifiers.
- Seven Expo methods retain platform-owned storage, identity and lifecycle
  admission: `prepareStorage`, `inspectIdentity`, `createGeneratedIdentity`,
  `createImportedIdentity`, `start`, `stop` and `reset`. Inputs and results cross
  Expo as owned byte arrays using the generated UniFFI codecs, without a
  handwritten domain model or field-by-field mapping.
- `prepareOutbound` retains the platform preflight before outbound work.
  Accessory setup, permissions and platform status events remain native concerns.

The synchronous generated `native*` functions are for Swift/Kotlin background
or lifecycle queues only. They can perform storage I/O or wait for Start/Stop;
JavaScript must use the Expo admission methods instead. Storage preparation
establishes the existing database owner before offline async access. Async calls
cannot supply filesystem paths or silently reopen storage after reset. Native
Start uses that same owner.

## Canonical values

Application counters and identifiers use Rust `u64`, generated as exact
TypeScript `bigint`, Swift `UInt64` and Kotlin `ULong`. Keep them exact through
arithmetic and comparisons. Canonical Host `safeUint` fields remain checked
JavaScript numbers, while canonical exact counters remain `bigint`.

`host_contract.py` resolves the app's locked `prns-host` dependency through Cargo
metadata and reads its canonical `host-contract-v1.json` schema. It generates
transport records and conversions over the actual Rust `HostSnapshot`, custom
type configuration and the TypeScript adapter to `personal-rns/contract`.
Canonical state remains owned by `prns_host`; the no_std core has no UniFFI
dependency. There is no second handwritten snapshot or JSON/ts-rs contract.

Generated types support `exactOptionalPropertyTypes`. UniFFI `None` uses
`undefined`; the canonical Host adapter omits absent optional properties as its
contract requires. Generated tagged enum variants use `.tag` and, for payloads,
`.inner`. Fixed 16/32-byte inputs validate during lifting. Contact normalization,
identity and pin rules, mailbox pagination and other input bounds remain shared
Rust semantic checks; TypeScript types alone do not validate foreign inputs.

## Cancellation and ownership

Generated async functions accept optional `{ signal: AbortSignal }`. The SDK
forwards caller signals, including Effect interruption, and checks cancellation
after platform preflight before submitting domain work. Aborting a caller or
tearing down its JavaScript runtime releases its generated future without
stopping the native node.

Describe cancellation drops its app-owned network future and releases admission;
its deadline and priority Stop also bound the operation. Cancellation cannot undo
upstream work already issued before a Link handle is available. Accepted contact
and mailbox mutations, sends and retained announcement operations remain owned
by their native lanes after the caller departs. Stop drains admitted durable
work. A rejected caller promise therefore does not prove that an accepted write
was rolled back and must not trigger an automatic retry.

Async admission uses bounded lanes and nonblocking supervisor access. It performs
no path resolution, database opening, synchronous response wait or worker join
on Hermes. A process-wide timer driver keeps offline query deadlines working;
it owns neither an executor nor a node.

## Runtime scope and validation

The runtime and generator use the exact jsi2 source revision and ordered patches
recorded in the [vendor distribution](../../../vendor/ubrn/README.md). The current
patches cover strict optional properties, JavaScript runtime teardown, Android
queue teardown and Apple framework version metadata. They are shared runtime
maintenance, separate from the generated application API; the vendor guide
documents verification and eventual replacement with an upstream release.

This API uses generated future-completion callbacks, not application-defined
UniFFI callback interfaces. Bluetooth callbacks and platform events retain their
existing native ownership. Adding callback interfaces or new runtime features
requires separate support and lifecycle validation.

`npm --prefix applications run native:test` covers full snapshot transport,
native-before-JavaScript ownership, exact integers, cold offline storage,
generation-safe deadlines, caller cancellation and durable writes after caller
drop. `api:check` verifies generated outputs. These host checks complement actual
iOS/Android startup, reload and lifecycle validation; they do not replace it.

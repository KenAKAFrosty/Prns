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

Generation refreshes product TypeScript, Swift, Kotlin and semantic contract
fingerprints, and selects this composition through `host-provider.json` for the
general Expo SDK. Checking rejects stale output without rewriting it. The SDK
package owns the sole `prns_app` native image; this package owns only product
bindings. The native client helpers build it before Expo prebuild/autolinking.
`bindings:aggregate:test` exercises real generated Python namespaces against
that image, including borrowed ownership, native event admission and restart;
the routine `native:verify` gate includes it.

See the [generation guide](../../../tools/generated-bindings/README.md) for
toolchain requirements, target selection and disposable build caches. Generated
files are outputs of that workflow and must not be edited directly.

## Imports and platform admission

Use `@prns-internal/native-bindings` for types, enum factories, generated
converters and fingerprint constants. This entry is safe to import on web and
does not install the native runtime. The `/native` entry installs the JSI runtime
and initializes the generated UniFFI ABI checks. The app-owned platform facade
loads it lazily after checking native capability, then checks the app and canonical
Host semantic fingerprints separately. Product calls use `@prns-internal/expo`
for platform admission; general host calls use `personal-rns-expo`.

The execution boundary is:

- Product snapshots, pairing/management workflows, contact operations and LXMF
  operations call generated async bindings. `previewIdentityImport` is a bounded synchronous
  parse of exactly 64 bytes; `bindingContract` reads immutable identifiers.
- Seven Expo methods retain platform-owned storage, identity and lifecycle
  admission: `prepareStorage`, `inspectIdentity`, `createGeneratedIdentity`,
  `createImportedIdentity`, `start`, `stop` and `reset`. Inputs and results cross
  Expo as owned byte arrays using the generated UniFFI codecs, without a
  handwritten domain model or field-by-field mapping.
- `prepareOutbound` retains the platform preflight before outbound work.
  Bluetooth authorization, permissions and platform status events remain native concerns.

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

The repository's canonical generator owns all host records, conversions and
the TypeScript adapter to `personal-rns/contract`. The former app snapshot
generator has been deleted. This package imports `personal-rns-expo` converters
and the external `HostClientHandle`; its custom snapshot type delegates to those
same generated conversions. Canonical state remains owned by `prns_host`; the
no_std core has no UniFFI dependency. `sharedHost` lends command/query access
without the authority to stop native services.

Generated types support `exactOptionalPropertyTypes`. UniFFI `None` uses
`undefined`; the canonical Host adapter omits absent optional properties as its
contract requires. Generated tagged enum variants use `.tag` and, for payloads,
`.inner`. Fixed 16/32-byte inputs validate during lifting. Contact normalization,
identity and pin rules, mailbox pagination and other input bounds remain shared
Rust semantic checks; TypeScript types alone do not validate foreign inputs.

## Cancellation and ownership

Generated async functions accept optional `{ signal: AbortSignal }`. The app facade
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
recorded in the [vendor distribution](../../../../vendor/ubrn/README.md). The current
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

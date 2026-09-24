# PRNS for Expo / React Native

`personal-rns-expo` exposes the general PRNS host through generated UniFFI/JSI
bindings. Expo is required. It has no dependency on the PRNS app, contacts, LXMF,
Effect, or a product runtime singleton. Configuration, commands, results,
snapshots, and events use `personal-rns/contract`. The handwritten wrapper owns
leases, iterators, and resource handles; generated adapters own value conversion.

```ts
import { balancedLimits, defaultStoragePath, openHost } from "personal-rns-expo";

const root = await defaultStoragePath();
const session = await openHost({
  identity: { tag: "LoadOrCreate", data: { path: `${root}/identity` } },
  persistence: { tag: "Directory", data: { path: `${root}/state` } },
  role: "Endpoint", destinations: [], requiredCapabilities: ["TcpClient"],
  limits: balancedLimits(),
});
const result = await session.host.execute({
  tag: "AttachTcpClient",
  data: { target: "192.168.1.10:4242", bitrate: { tag: "Auto", data: undefined } },
});
const snapshot = await session.host.snapshot();
await session.stop();
```

`OwnedHostSession` carries shutdown authority. Its `host` is a borrowed
`HostClient` with commands, inspection, event streams, and resource uploads.
`HostClient.release()` releases its streams/uploads and lease; it cannot stop an
app's composed host or bypass native service draining. Native extensions can
return the SDK's generated `HostClientHandle` external type and call `borrowHost` in JS.
They must use the SDK's converter, not generate another host wrapper.

`stop()` and `close()` share one completion. A joined native shutdown failure
still rejects with `BindingError.Backend`, releases the JS handles, and preserves
that same failure for subsequent callers. `BindingError.OwnershipUnavailable` or
an unrecognized transport failure retains ownership so shutdown can be retried.
Snapshot contention and stopped hosts retain their typed `Busy` and `Stopped`
errors.

`execute` preserves typed protocol outcomes. Binding input, ownership, and
runtime errors remain generated typed errors. Remote control uses
`remoteControlExchange(linkId, request)` or
`remoteControlTargetExchange(targetIdentityHash, request)` with the generated
`personal-rns/remote-control` request, response, and failure types. No operation
list is maintained by hand in the SDK. Aborting a command wait does not
roll back an operation already admitted by Rust.

`SendLinkPacket` preserves the core's delivery-proof behavior. Packets sent to a
`ProveAll` responder settle with an explicit delivery proof. Raw packets sent
back to the link initiator can arrive as `LinkDelivery` events while the
responder's command settles as `Failed(DeliveryTimedOut)`: the initiator does not
emit an automatic packet proof. A timeout therefore does not establish that the
peer received no bytes. Applications that need confirmed replies must account
for this core behavior; the SDK does not add an acknowledgement protocol.

Destination public-key lookup is available on the generated native
`HostClientHandle.destinationPublicKey` binding. Authenticated announce callbacks
and prepared native transport objects remain Rust embedding hooks; they are not
part of the React Native event API. C, N-API and browser projections do not expose
these native extensions. The authoritative source inventory and generated target
matrix live in `prns-host/schema/native-extensions-v1.json` and
`prns-host/native-extensions.generated.md`; generator stale checks cover them.
Foreign authenticated-observation parity remains an explicit limitation.

## Events and resources

```ts
const abort = new AbortController();
for await (const event of session.host.applicationEvents({ signal: abort.signal })) {
  // Process a canonical ApplicationEvent.
}
```

Only one consumer owns each application or diagnostic lane. A native service
may already own it; a second claim fails explicitly. The iterator awaits
non-consuming readiness, checks liveness, then performs one bounded synchronous
drain. Abort, return, or failure closes the claim. Concurrent `next()` calls are
rejected. JS contains no second queue, replay policy, or broadcast implementation.

Resource events contain the canonical `ResourceStream`: call `claim()` and read
the returned iterator. Chunks are at most 64 KiB; returning the iterator releases
the reader. Received resources own their bytes independently of the client.
`beginResourceUpload` returns `writeChunk`, `finish`, and `abort`; native bounds
and backpressure remain authoritative. Durable services must consume and persist
in Rust; delivery to JS does not promise recovery after runtime destruction.

## Select one native image

The source checkout uses the single runtime distribution under `vendor/ubrn` and
the shared `tools/uniffi` generator. From the repository root:

```sh
python3 prns-react-native/tools/generate.py generate
python3 prns-react-native/tools/generate.py generate --check
python3 prns-react-native/tools/generate.py android --targets arm64-v8a
python3 prns-react-native/tools/generate.py ios --sim-only --targets aarch64-apple-ios-sim
```

`PRNS_UBRN_CACHE` selects disposable generator cache; `CARGO_TARGET_DIR` selects
the Rust build cache. `providers/default.json` builds the SDK's default
`prns-host-uniffi-image` as `prns_host_mobile`.

An app with native services supplies `--provider /path/to/provider.json`. That
versioned document contains exactly one provider, whose aggregate crate depends
on the SDK Rust `rlib` and adds service/product exports. Generation applies the
selected image name to JSI, Kotlin, and Expo without collapsing distinct native
namespaces. Pod/Gradle rules reject extra or missing images in this package.
Remove separate packaging of the previous provider when switching; the app must
not embed another SDK/default image through another package. SDK converters and
the public JS API are shared between default and aggregate providers.

Provider source paths resolve relative to the JSON file and are absent from the
distributed `native-image.json`. Provider `features` apply to native builds and
binding generation; `generationFeatures` enable tools such as `uniffi-bindgen`
only while generating bindings. The semantic fingerprint and UniFFI checksums
are checked before host use. A changed contract/provider requires a native
rebuild and the corresponding Expo update runtime-version change.

## Independent example and web

`example/` is a separate Expo project that opens a persistent host, inspects it,
connects a TCP peer, and stops it. Build the SDK image first, install the example's
dependencies, then run `npm run android` or `npm run ios` from that directory.
A custom development build is required; Expo Go cannot load this Rust image.
The peer address is editable. No PRNS product/service package is used.
The example enables `expo-build-properties` → `ios.enableSceneSupport` so Expo
owns scene startup on iOS 27. Consumers built with that SDK must also configure
scene startup; this is an Expo app setting, not a PRNS runtime service.

The browser entry delegates directly to the existing backend:

```ts
import { BrowserHost, persistentBrowser } from "personal-rns-expo/browser";
const outcome = await BrowserHost.create(persistentBrowser("my-node"));
```

For the Expo web example, build and stage the existing browser WASM package
(`npm --prefix prns-wasm run build:wasm`, then `node prns-js/scripts/stage.mjs browser`
from the repository root) and run `npm run export:web` in `example/`. Its staging
helper copies the installed core package's WASM assets into the public directory;
Expo bundles the existing PRNS workers. Serve the resulting `example/dist/` to
exercise persistent browser startup and shutdown. WASM and JavaScript must come
from the same contract version.

Browser creation retains its existing worker/storage options and explicit
unsupported-capability outcomes. Native filesystem configuration and TCP are
not emulated. The native SDK imports only `personal-rns/contract`, avoiding its
backend-selecting root entry point.

## Qualification

See the [standalone qualification record](docs/qualification.md) for executed
checks and the remaining device/publication limits.

Strict TypeScript checking covers the generated binding/adapter, SDK native/web
exports, and example. Iterator tests cover abort before readiness, abort after
wake before drain, return during a pending wait, concurrent waits, and failure.
Session tests cover joined failure cleanup, shared stop outcomes and retry after
unavailable ownership. Provider tests reject missing/duplicate image selections.
`npm run verify` runs the TypeScript, JavaScript and provider checks.

For a clean source checkout, build the core contract and install the SDK's locked
development dependencies from the repository root:

```sh
npm --prefix prns-js ci --ignore-scripts
npm --prefix prns-js run build:code
npm --prefix prns-react-native ci --ignore-scripts
npm --prefix prns-react-native run verify
```

This setup uses the core package and pinned runtime archives without an app
workspace install. Dedicated SDK CI jobs verify the generated default provider,
contract conformance, package checks, and native builds independently.

`npm run test:consumer`
installs the packed SDK, core package, and pinned runtime archives outside this
checkout, then checks the example and compiles the packaged Android SDK. It
requires a built default Android image. To check an already-built aggregate image,
run `node tools/detached-consumer.mjs --aggregate --android --keep` from this
package. Add `--bindings /absolute/path/to/composition/bindings` to pack and
TypeScript-check the composition's native export against the same SDK. The
consumer uses npm overrides to resolve private workspace dependencies to those
exact packed SDK/core/runtime inputs; the original archives remain unchanged.
Both modes reject missing, duplicate, or mismatched Android images. `--keep`
retains the consumer, archive hashes, installed image hashes, and validation log.
These checks compile packages; they do not build a new APK or replace device
qualification.

After building both native targets, `npm run pack:check` checks the actual npm
archive for the selected Android image and iOS XCFramework, including each
framework binary and its metadata. Use `npm run pack:check -- --aggregate --keep`
for an aggregate provider and a retained archive/receipt. The check does not
compile or install a mobile app, and a simulator-only framework only qualifies
simulator packaging. The full consumer command also validates any included Apple
framework; add `--require-ios` to reject an absent framework.

On macOS, `node tools/detached-consumer.mjs --ios --keep` builds the packed SDK's
complete Expo example for an Apple Silicon simulator, including the
CocoaPod and generated Swift. It requires a matching simulator framework,
Xcode and CocoaPods. Its receipt reports `iosSdkCompilation` separately from
archive contents and device qualification. Temporary dependencies, caches and
build outputs stay inside the detached consumer. Dedicated SDK CI runs both the
Android consumer compile and this unsigned iOS consumer build.

The Expo module checks the image and supplies a private storage path. Shared
Android BLE/GATT pumps, bridge ownership, startup admission, radio recovery,
and foreground-service lifetime live here. Shared iOS authorization, startup
dispatch, restoration admission, and protected-data recovery live here too.
Bluetooth permissions, background behavior, and iOS restoration require device
qualification. Desktop capability advertisements alone do not qualify mobile
transports. Native builds and device results are reported separately.

## Native retained ownership

For Android background operation, subclass `PrnsForegroundService` and provide
one process-owned `PrnsForegroundRuntime` plus a `PrnsRuntimeDelegate` from
`createDelegate()`. Android constructs that factory before JavaScript, including
service restoration. The SDK owns admission, serial native lifecycle work,
Bluetooth workers, ordered drain, and foreground lifetime. React context teardown
does not stop the owner. A failed drain retains ownership until a later stop
succeeds.

The delegate supplies native start/stop operations, saved nonsecret startup
configuration, a foreground notification, and its notification identifier/type.
It returns opaque codec responses plus a running/stopped acknowledgement. A
`stopped` acknowledgement must mean native work has completely drained; the SDK
then releases Bluetooth transport. The consuming app retains its own storage
policy, reset workflow, and notification copy. Apps without Bluetooth set
`bluetoothEnabled` to false.
Declare the concrete service and required foreground-service permissions in the
consuming app manifest. No product service or notification is auto-registered.

The delegate may keep a generated `HostSession` in its native process owner and
return borrowed `HostClientHandle` handles from a native module. JavaScript wraps those with
`borrowHost`; only the native owner calls `HostSession.stop()`. Keep raw identity
secrets in native storage, never in service intents or persisted startup bytes.
The minimal example uses explicit JS-owned sessions; retained/background
ownership requires a configured native service factory as described above.

Remote Control is an optional host service. Pass `openHost(config, { remoteControl })` with caller-selected controller/target identity sources, initial grants, announcement policy and capabilities. The SDK adds no product directories or default grants. `host.remoteControl` exposes generated pairing-window, pairing-decision and target-access operations. Pairing observations arrive as `RemoteControl` cases on the same `applicationEvents()` iterator and use its existing queue limits and sole-consumer claim. Remote Control result, error, configuration and observation types are generated from the public Rust declarations.

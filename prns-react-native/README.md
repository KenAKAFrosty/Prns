# PRNS for Expo / React Native

`personal-rns-expo` exposes the general PRNS host through generated UniFFI/JSI
bindings. Expo is required. It has no dependency on the PRNS app, contacts, LXMF,
Effect, or a product runtime singleton. Configuration, commands, results,
snapshots, and events use `personal-rns/contract`. The handwritten wrapper owns
leases, iterators, and resource handles; generated adapters own value conversion.

This is a source preview. The SDK and its patched runtime archives are not yet
available as a complete public-registry installation. Use this repository's
locked packages and the source workflow below. A custom native build is required;
Expo Go cannot load the Rust image.

## Source quickstart

Run the commands below from the repository root. Use Node 24.18.0, npm 11.16.0,
Python 3.11+ and Rust through rustup. Native builds use Rust 1.98.1; the reviewed
native toolchain versions are recorded in
[`vendor/ubrn/source-lock.json`](../vendor/ubrn/source-lock.json).
On Linux, install `libdbus-1-dev` and `pkg-config` for host metadata generation.

First install the locked dependencies and build the core JavaScript contract:

```sh
npm install --global npm@11.16.0
rustup toolchain install 1.98.1 --profile minimal
export RUSTUP_TOOLCHAIN=1.98.1
npm --prefix prns-js ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-js run build:code
npm --prefix prns-react-native ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-react-native run verify
python3 prns-react-native/tools/generate.py generate --check
```

This setup uses the core package and pinned runtime archives without an app
workspace install. The shared `tools/uniffi` generator uses the single runtime
distribution under `vendor/ubrn`. `providers/default.json` selects
`prns-host-uniffi-image`, which builds the native image `prns_host_mobile`.
`PRNS_UBRN_CACHE` selects disposable generator cache; `CARGO_TARGET_DIR` selects
the Rust build cache.

Then build the SDK image for the platform you will run. For Android, install
Java 17 and the Android command-line tools, and set `ANDROID_HOME` to the SDK
directory. This builds for an arm64 device or emulator:

```sh
sdkmanager "platforms;android-36" "build-tools;36.0.0" "ndk;27.1.12297006"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.1.12297006"
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked --version 4.1.2
python3 prns-react-native/tools/generate.py android --targets arm64-v8a
```

For an x86_64 emulator, add the `x86_64-linux-android` Rust target and use
`--targets x86_64` instead. On macOS, install Xcode 27.0 and CocoaPods; this
builds for an Apple Silicon iOS simulator:

```sh
rustup target add aarch64-apple-ios-sim
python3 prns-react-native/tools/generate.py ios --sim-only --targets aarch64-apple-ios-sim
```

For a physical iOS device, also add the `aarch64-apple-ios` Rust target and build
with `ios --targets aarch64-apple-ios,aarch64-apple-ios-sim` instead. Installing
the example on a device requires your own app signing configuration.

Finally install the independent example and run it on your chosen platform:

```sh
npm --prefix prns-react-native/example ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-react-native/example run android -- --device
# Or, for the iOS simulator:
npm --prefix prns-react-native/example run ios
```

`example/` opens a persistent host, inspects it, connects to an editable TCP peer
address, and stops it. No PRNS product/service package is used. The example
enables `expo-build-properties` → `ios.enableSceneSupport` so Expo owns scene
startup on iOS 27. Consumers built with that SDK must also configure scene
startup; this is an Expo app setting.

## Open a host

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

## Native compositions: select one image

An app with native services supplies `--provider /path/to/provider.json` and
`--destination /path/to/app/target/host-sdk`. The versioned provider document
contains exactly one aggregate crate, which depends on the SDK Rust `rlib` and
adds service/product exports. The shared generator copies the SDK's maintained
package sources into that disposable destination and generates its selected
JSI, Swift, and Kotlin bindings there. It leaves this source package's default
provider unchanged. The staged runtime manifest retains the public peer
contract and omits development dependencies and scripts.

```sh
python3 prns-react-native/tools/generate.py \
  --provider /path/to/provider.json --destination /path/to/app/target/host-sdk generate
python3 prns-react-native/tools/generate.py \
  --provider /path/to/provider.json --destination /path/to/app/target/host-sdk generate --check
python3 prns-react-native/tools/generate.py \
  --provider /path/to/provider.json --destination /path/to/app/target/host-sdk android --targets arm64-v8a
```

Select that same staged package in every app consumer, and generate it before
installing file dependencies. Checking rejects missing or stale staging without
rewriting it. Native builds write into the destination and preserve existing
artifacts for the same selected image. Choose a clean destination to switch
images. Pod/Gradle rules reject extra or missing images; the app must not embed
another SDK/default image through another package. SDK converters and the public
JS API have one maintained implementation shared by both provider modes.

Provider source paths resolve relative to the JSON file and are absent from the
distributed `native-image.json`. Provider `features` apply to native builds and
binding generation; `generationFeatures` enable tools such as `uniffi-bindgen`
only while generating bindings. The semantic fingerprint and UniFFI checksums
are checked before host use. A changed contract/provider requires a native
rebuild and the corresponding Expo update runtime-version change.

## Browser

The browser entry delegates directly to the existing backend:

```ts
import { BrowserHost, persistentBrowser } from "personal-rns-expo/browser";
const outcome = await BrowserHost.create(persistentBrowser("my-node"));
```

After the source quickstart's dependency setup and Rust toolchain selection,
install the WebAssembly target and the CLI version matching `prns-wasm/Cargo.lock`.
Build, stage and export the Expo web example from the repository root:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked --version 0.2.126
env -u CARGO_TARGET_DIR npm --prefix prns-wasm run build:wasm
node prns-js/scripts/stage.mjs browser
npm --prefix prns-react-native/example ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-react-native/example run export:web
```

The WASM build script expects its local target directory, so that command clears
any shared `CARGO_TARGET_DIR`. The example's staging helper copies the installed
core package's WASM assets into the public directory; Expo bundles the existing
PRNS workers. Serve `prns-react-native/example/dist/` to exercise persistent
browser startup and shutdown. WASM and JavaScript must come from the same
contract version.

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

After the source quickstart, dedicated SDK CI jobs verify the generated default
provider, contract conformance, package checks, and native builds independently.
The commands below run from `prns-react-native/`.

`npm run test:consumer`
installs the packed SDK, core package, and pinned runtime archives outside this
checkout, then checks the example and compiles the packaged Android SDK. It
requires a built default Android image. To check an already-built aggregate image,
run `node tools/detached-consumer.mjs --sdk /path/to/app/target/host-sdk --aggregate --android --keep`
from this source package. Add `--bindings /absolute/path/to/composition/bindings` to pack and
TypeScript-check the composition's native export against the same SDK. The
consumer uses npm overrides to resolve private workspace dependencies to those
exact packed SDK/core/runtime inputs; the original archives remain unchanged.
Both modes reject missing, duplicate, or mismatched Android images. `--keep`
retains the consumer, archive hashes, installed image hashes, and validation log.
These checks compile packages; they do not build a new APK or replace device
qualification.

After building both native targets, `npm run pack:check` checks the actual npm
archive for the selected Android image and iOS XCFramework, including each
framework binary and its metadata. Use `npm run pack:check -- --sdk /path/to/app/target/host-sdk --aggregate --keep`
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

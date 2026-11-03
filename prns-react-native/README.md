# PRNS for Expo / React Native

`personal-rns-expo` exposes the general PRNS host through generated UniFFI/JSI
bindings. Expo is required. It has no dependency on the PRNS app, contacts, LXMF,
Effect, or a product runtime singleton. Configuration, commands, results,
snapshots, and events use `personal-rns/contract`. The handwritten wrapper owns
leases, iterators, and resource handles; generated adapters own value conversion.

This SDK is a source preview. Use this repository's locked packages and the
source workflow below. Its `@ubjs` runtime peers are published npm packages that
require the SDK's [runtime patch](#required-runtime-patch). A custom native build
is required; Expo Go cannot load the Rust image.

## Ownership and source map

| Surface | Owner |
| --- | --- |
| Host values and contract | `personal-rns/contract` and generated adapters in `src/generated/` |
| JavaScript leases, streams and resources | `src/index.native.ts` and `src/stream.ts` |
| Selected native image | `providers/default.json`, `tools/generate.py`, and `outputs.json` |
| Android lifecycle, Bluetooth and LAN | `android/src/main/` plus the JNI bridge in `platform/android/` |
| iOS authorization, restoration and path changes | `ios/Prns*.swift`; native transport construction remains host-owned |
| Package and consumer checks | `tools/detached-consumer.mjs`, `tools/package_notices.py`, `tests/` and platform tests |

The default provider selects the reusable host image. Product compositions supply
another provider and use the same SDK implementation; they do not add domain
methods to its JavaScript wrapper. Saved connection profiles and Remote Control
are host capabilities, while the SDK supplies mobile callbacks and ownership
mechanics. Notification copy, installation policy, startup settings and storage
locations remain consumer choices. See the repository's
[capability overview](../docs/composable-features.md) and the
[qualification guide](docs/qualification.md).

## Source quickstart

Run the commands below from the repository root. Use Node 24.18.0, npm 11.16.0,
Python 3.11+ and Rust through rustup. The SDK and example use Expo 58,
React Native 0.88.1 and React 19.3.0. Native builds use Rust 1.98.1; the reviewed
native toolchain versions are recorded in
[`tools/uniffi/react-native.json`](../tools/uniffi/react-native.json).
On Linux, install `libdbus-1-dev` and `pkg-config` for host metadata generation.

First install the locked dependencies and build the core JavaScript contract:

```sh
npm install --global npm@11.16.0
rustup toolchain install 1.98.1 --profile minimal
export RUSTUP_TOOLCHAIN=1.98.1
npm --prefix prns-js ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-js run build:code
npm --prefix prns-js run check:built-contracts
npm --prefix prns-react-native ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-react-native run patch:runtime
npm --prefix prns-react-native run verify
python3 prns-react-native/tools/generate.py generate --check
```

Rebuild `prns-js` after regenerating either contract. The example's Metro
configuration checks the compiled public host and Remote Control exports against
the generated source before bundling. Native runtime compatibility checks remain
enabled.

This setup uses the core package and exact registry runtime versions without an
app workspace install. The shared `tools/uniffi` generator selects its source and
patches through `tools/uniffi/react-native.json`. `providers/default.json` selects
`prns-host-uniffi-image`, which builds the native image `prns_host_mobile`.
`PRNS_UBRN_CACHE` selects disposable generator cache; `CARGO_TARGET_DIR` selects
the Rust build cache.

Then build the SDK image for the platform you will run. For Android, install
Java 17 and the Android command-line tools, and set `ANDROID_HOME` to the SDK
directory. This builds for an arm64 device or emulator:

```sh
sdkmanager "platforms;android-37.0" "build-tools;37.0.0" "ndk;27.1.12297006"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.1.12297006"
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked --version 4.1.2
python3 prns-react-native/tools/generate.py android --targets arm64-v8a
```

For an x86_64 emulator, add the `x86_64-linux-android` Rust target and use
`--targets x86_64` instead. On macOS, install Xcode 27.1 and CocoaPods 1.16.2 with a supported modern Ruby; this
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
npm --prefix prns-react-native/example run patch:runtime
npm --prefix prns-react-native/example run android -- --device
# Or, for the iOS simulator:
npm --prefix prns-react-native/example run ios
```

`example/` opens a persistent host, inspects it, connects to an editable TCP peer
address, and stops it. No PRNS product/service package is used. Expo 58's native
project template supplies iOS scene startup through the app's
`SceneDelegate` subclass of `ExpoAppSceneDelegate`. Consumers should retain that
template integration; no additional scene-support build property is required.

## Required runtime patch

Use exact `@ubjs/core` and `@ubjs/react-native` version `0.31.0-6`. The SDK ships
[`patches/@ubjs+react-native+0.31.0-6.patch`](patches/@ubjs+react-native+0.31.0-6.patch)
for runtime cleanup, native object lifetime handling and Apple framework version
metadata. It changes text sources and metadata; published native binaries remain
unchanged.

The consuming app must apply this patch after installing dependencies and before
building native code. Add `patch-package` version `8.0.1` to the app's development
dependencies, then run from the application's package root:

```sh
node -e "require('node:fs').accessSync('node_modules/personal-rns-expo/patches/@ubjs+react-native+0.31.0-6.patch')" &&
  npm exec --offline -- patch-package --error-on-fail --patch-dir node_modules/personal-rns-expo/patches
```

The file check catches a missing payload: patch-package otherwise treats an empty
patch directory as success. Repeat application is safe; an incompatible required
hunk fails. An app may put this command in its own `postinstall` script, as the
[example](example/README.md) does. With `--ignore-scripts`, run it explicitly.
The SDK library has no installation hook. Source SDK development uses
`npm run patch:runtime` from this directory.

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

The generated `HostClientHandle.inspection()` extension returns the logical host
snapshot, physical interface rows and the engine clock used by route timestamps.
Physical rows retain parent/control associations, endpoint, radio and peer-detail
facts; their counters must not be added to folded logical counters. This extension
does not infer physical connectivity from routes or Link activity.

Android consumers can opt into `PrnsBluetoothRuntimeDelegate` to replace only a
Bluetooth attachment. The SDK waits for detach before retiring its listener and
prepared bridge, then reattaches under the same service admission ticket. Stop
supersedes every continuation. Consumers without this delegate retain their
existing complete-runtime reconnect path. Permission prompts and durable
transport preferences remain application policy.

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
npm --prefix prns-wasm run build:wasm
node prns-js/scripts/stage.mjs browser
npm --prefix prns-react-native/example ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-react-native/example run export:web
```

The WASM build script resolves Cargo's output directory and honors a shared
`CARGO_TARGET_DIR`, including an external build cache. Generated package assets
stay in `prns-wasm/pkg`. The example's staging helper copies the installed core
package's WASM assets into the public directory; Expo bundles the existing PRNS
workers. Serve `prns-react-native/example/dist/` to exercise persistent browser
startup and shutdown. WASM and JavaScript must come from the same contract
version.

Browser creation retains its existing worker/storage options and explicit
unsupported-capability outcomes. Native filesystem configuration and TCP are
not emulated. The native SDK imports only `personal-rns/contract`, avoiding its
backend-selecting root entry point.

## Qualification

See the [qualification guide](docs/qualification.md) for repeatable checks,
phone acceptance and the remaining publication requirements.

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
installs the packed SDK and core package alongside the exact registry runtime
versions outside this checkout. It verifies the runtime lockfile URLs and
integrities, applies the SDK-shipped patch, then checks the example and compiles
the packaged Android SDK. It
requires a built default Android image. To check an already-built aggregate image,
run `node tools/detached-consumer.mjs --sdk /path/to/app/target/host-sdk --aggregate --android --keep`
from this source package. Add `--bindings /absolute/path/to/composition/bindings` to pack and
TypeScript-check the composition's native export against the same SDK. The
consumer uses npm overrides to resolve private workspace dependencies to those
exact packed SDK/core and registry runtime inputs.
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

Android GATT operation state and callback ownership have one maintained source
in `prns-host/platform/android/bluetooth`. The generator copies its exact sources
and shared tests into the SDK's owned Android output directories. Published
packages remain self-contained; Hopspot compiles the generic sources directly
without depending on Expo. Writes and
notifications have a 30-second progress deadline from the first attempt;
persistent Busy retries do not reset it. Expiration closes the physical link
rather than reusing an uncertain operation lane. A timed-out peripheral address
stays reserved until the GATT
server is reset, because Android's server callbacks do not identify the original
connection. This protects replacement ownership; it does not by itself provide
automatic recovery from an idle peer's process restart.

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

Existing `PrnsRuntimeDelegate` implementations retain their synchronous contract.
For native owners with asynchronous readiness and close, implement the opt-in
`PrnsAsyncRuntimeDelegate` completion overloads. Each entrypoint must return
promptly; observe readiness/drain on the native owner's executor or an observer
coroutine. Stop must fence a pending Start immediately, and repeated Stop must
join the existing close. Calling the synchronous entrypoints on this new
capability is unsupported and throws instead of silently blocking the lifecycle
lane.

The SDK marshals one completion back to its existing lifecycle executor, even
when a callback is synchronous, duplicated, or followed by a thrown exception.
This applies to startup, explicit stop/reset, failed-start cleanup, Bluetooth
reconnection, and service destruction. Delayed startup/reconnection callbacks
must still match the current owner and admission ticket. Stop also fences
replacement while no Android service exists; a failed drain keeps that fence,
and a newer Stop preserves any outstanding Reset request. Transport release
requires native drain followed by the SDK's physical worker drain. Saved-input
failures do not prevent the native Stop entrypoint from being invoked. The
readiness check admits a completion to the serial lane; a Stop racing an already
admitted completion clears its saved input before acknowledging shutdown. No
delegate callback runs under the admission lock. Successful asynchronous Stop
acknowledgement covers restoration-input clearing and complete drain. Process
exit before that acknowledgement may retain the previous restoration intent;
crash durability at Stop invocation requires a separate product storage policy.

For asynchronous delegates, `PrnsRuntimeStart(running = true, error = message)`
means the native owner is ready but an optional transport failed. The SDK saves
restoration input, reports running with that error, and rejects the individual
request with `ERR_PRNS_TRANSPORT`; it keeps the ready owner alive. This meaning is
opt-in and does not alter synchronous delegates' result handling.

The delegate may keep a generated `HostSession` in its native process owner and
return borrowed `HostClientHandle` handles from a native module. JavaScript wraps those with
`borrowHost`; only the native owner calls `HostSession.stop()`. Keep raw identity
secrets in native storage, never in service intents or persisted startup bytes.
The minimal example uses explicit JS-owned sessions; retained/background
ownership requires a configured native service factory as described above.

### Remote Control

Remote Control is an optional host service. Pass `openHost(config, { remoteControl })` with caller-selected controller/target identity sources, initial grants, announcement policy and capabilities. The SDK adds no product directories or default grants. `host.remoteControl` exposes generated pairing-window, pairing-decision and target-access operations. Pairing observations arrive as `RemoteControl` cases on the same `applicationEvents()` iterator and use its existing queue limits and sole-consumer claim. Remote Control result, error, configuration and observation types are generated from the public Rust declarations.


## Aggregate native namespace ownership

A staged SDK for a selected aggregate image compiles every generated Swift and
Kotlin namespace from that image. This keeps cross-namespace Swift types in the
single `PrnsHostExpo` module and preserves Kotlin's owning package imports. A
product platform imports those declarations; it must not compile a second copy
of the same generated sources or package another native image. The standalone
SDK remains independent of application code and selects only its default image.

TypeScript remains split by owner: this SDK contains its canonical host module
and converter; compositions generate their other namespaces and re-export the
host converter from `personal-rns-expo/bindings/values`. No domain methods are
filtered, copied into a facade, or listed in the SDK's image selection logic.


## Selected image attribution

`npm run notices:generate` records the locked shipping closure of the default image
and the exact runtime peer archives in `THIRD_PARTY_NOTICES.md` and
`third-party-sources.json`. `npm run notices:check` reproduces the material through
pinned cargo-about 0.9.1, using a fresh locked dependency fetch; `notices:inputs`
checks the recorded inputs without regenerating the full notices, but may fetch
missing runtime archives or Cargo dependencies. The detached package checks inspect both
files inside the actual archive, verify the selected image and notice hash, and
verify the installed registry runtime versions, tarball URLs and integrity values
before applying the packaged runtime patch.

For an aggregate, use the same tool with `--provider PATH --destination PATH
--write` after generating that selected SDK stage. Staging deliberately does not
copy default-image attribution into another image. The source record includes the
upstream runtime revision, original notice, registry tarball integrity and source
patches. Runtime packages are separately installed peers; other peer declarations do not establish
that their code is embedded in the SDK.

This attribution is not license approval. The two `@ubjs` runtime packages declare
MPL-2.0, for which the existing npm release policy has no approved exception.
`npm-production-audit.py` reports that finding explicitly; package, build and
consumer checks do not waive it or grant distribution acceptance.

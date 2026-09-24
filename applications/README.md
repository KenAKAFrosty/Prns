# Prns applications

This workspace owns the Prns app, its native composition and reusable application
services. It consumes public core APIs; the core does not depend on this tree.

## Start here

- [App behavior and routes](prns/app/README.md)
- [iOS development](docs/ios.md) and [Android development](docs/android.md)
- [Current validation and limits](docs/validation.md)
- [React Native SDK ownership and implementation](docs/react-native-sdk-implementation.md)
- [Implementation roadmap](docs/roadmap.md)
- [Expanded remote-control plan](docs/remote-control-expansion.md)
- [Proposed two-phone local-node demo](docs/phone-node-demo.md)

This guide owns setup and workspace commands. The binding and platform guides
own their narrower boundaries; dated checkpoints preserve historical evidence,
not current setup instructions. The original scratch plans are design history.

## Ownership

| Path | Responsibility |
| --- | --- |
| `prns/app` | Expo screens, navigation and application UI |
| `prns/native-composition/src` | Rust application policy, storage, service composition and ownership of the shared host session |
| `prns/native-composition/bindings` | Generated product interface; imports shared host converters and borrows the SDK-selected native image |
| `prns/platform` | App storage/startup/reset admission, notification presentation and product platform facade; delegates generic lifecycle and Bluetooth mechanics to the SDK |
| `services` | Reusable application services, including LXMF and its wire format |
| `../prns-host/impls/native` | Shared native runtime, queues/resources, readiness and joined shutdown used by C, UniFFI and native services |
| `../prns-react-native` | Canonical general Expo SDK with its standalone default native image |
| `target/react-native-sdk` | Generated app-selected SDK package, with the app aggregate native image; no separately maintained SDK sources |
| `tools/generated-bindings` | App generation recipe using shared `../tools/uniffi` orchestration |
| `../vendor/ubrn` | Shared pinned upstream runtime archives, patches and provenance |

Start with the [binding boundary](prns/native-composition/bindings/README.md) for
generated values, cancellation and native ownership. Edit Rust declarations and
generator inputs, then regenerate; do not edit generated language bindings.

## Setup

Use the Node, npm, Python and rustup toolchain recorded in
[`release/compatibility.json`](release/compatibility.json). Install the recorded
Rust components and targets before running the full verification gates. Linux
native checks also require `pkg-config` and the DBus development headers
(`libdbus-1-dev` on Ubuntu).

From the repository root, build the core JavaScript contract and install the
application workspace:

```sh
npm --prefix prns-js ci --ignore-scripts --no-audit --no-fund
npm --prefix prns-js run build:code
npm --prefix prns-react-native ci --ignore-scripts --no-audit --no-fund
python3 applications/tools/generated-bindings/generate.py stage
npm --prefix applications ci --ignore-scripts --no-audit --no-fund
```

The app consumes `target/react-native-sdk`, staged from the canonical SDK through
its shared generator. `stage` checks the committed app bindings and writes only
the ignored package. Run it before installing dependencies in a fresh checkout,
and again after removing `applications/target`. The SDK's own development
dependencies support its independent validation.

Staging requires the recorded Rust toolchain and host compiler. Building mobile
images additionally requires Xcode for iOS builds and Swift tests. Android builds require
JDK 21, the Android SDK, the pinned NDK, `cargo-ndk` and the requested Rust targets;
see the [generation guide](tools/generated-bindings/README.md) and
[Android setup](docs/android.md#build).

## Generate, build and check

Run these commands from the repository root:

```sh
npm --prefix applications run api:generate
npm --prefix applications run api:check
npm --prefix applications run verify
npm --prefix applications run mobility:verify -- --working-tree
```

`verify` runs the portable Rust, generated-output, SDK, aggregate foreign-object
sharing, UI and web checks.
The canonical SDK keeps the `prns_host_mobile` provider; app generation and builds
select `prns_app` only in the staged package. Both packages can be checked in the
same checkout without switching providers or changing tracked SDK files.
`mobility:verify -- --working-tree` checks an isolated export of the current
source, including packed npm dependencies and native/Python LXMF interoperability.
It is the app PR gate. Without `--working-tree`, the command checks the recorded
release revision; that separate qualification requires the
[release record](release/README.md) to be promoted to a real SDK commit.
On macOS, also run the explicit
Swift lifecycle and release-symbol tests:

```sh
npm --prefix applications run native:ios:test
```

Build the complete app with embedded JavaScript, without installing or launching
it:

```sh
npm --prefix applications run native:android:standalone
npm --prefix applications run native:ios:build
```

Android produces a locally debug-signed Release APK with the development
identifier. iOS requires macOS, Xcode and CocoaPods; it compiles an unsigned
Release app for a generic Apple Silicon iOS simulator. Neither command starts
Metro or qualifies production distribution. The helpers refresh generated
bindings and verify the single aggregate image and embedded JavaScript; iOS also
checks scene/Bluetooth metadata.

The `application-mobile-build` CI matrix runs these commands as
`application-android-build` and `application-ios-build`, then checks that app
builds preserve the canonical SDK and committed product bindings. This is full
app build coverage alongside the standalone SDK jobs; see CI results for the
actual run outcome. Detached package installation and physical acceptance remain
separate checks.

Use `native:ios:client` or `native:android:client` for development clients that
use Metro. The iOS client helper also installs and runs its simulator smoke;
physical iOS installation uses `native:ios:device`. See the
[iOS](docs/ios.md) and [Android](docs/android.md) guides for those workflows.

`PRNS_UBRN_CACHE` and `CARGO_TARGET_DIR` can place disposable source/build caches
on a larger volume. Keep separate caches for different host operating systems.
The [validation record](docs/validation.md) distinguishes automated checks,
build-specific device observations and remaining release qualification.

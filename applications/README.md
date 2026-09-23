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
| `../prns-react-native` | General Expo SDK, generated host bindings, reusable mobile mechanics and packaging of the one selected native image |
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
npm --prefix applications ci --ignore-scripts --no-audit --no-fund
```

The SDK has its own locked development dependencies. Install them even when
working only on the app: platform typechecking follows the SDK source package.

The committed runtime archives make installation independent of mobile build
tools. Xcode is required for iOS builds and Swift tests. Android builds require
JDK 21, the Android SDK, the pinned NDK, `cargo-ndk` and the requested Rust targets;
see the [generation guide](tools/generated-bindings/README.md) and
[Android setup](docs/android.md#build).

## Generate, build and check

Run these commands from the repository root:

```sh
npm --prefix applications run api:generate
npm --prefix applications run api:check
npm --prefix applications run verify
npm --prefix applications run mobility:verify
```

`verify` runs the portable Rust, generated-output, SDK, aggregate foreign-object
sharing, UI and web checks.
`mobility:verify` checks a clean tracked export against the exact core revision
and includes native/Python LXMF interoperability. On macOS, also run the explicit
Swift lifecycle and release-symbol tests:

```sh
npm --prefix applications run native:ios:test
```

Build development clients with `native:ios:client` or `native:android:client`.
`native:android:standalone` embeds JavaScript in a development APK. These helpers
refresh generated bindings before packaging the shared Rust image. See the
[app guide](prns/app/README.md) for platform behavior and physical iOS installation.

`PRNS_UBRN_CACHE` and `CARGO_TARGET_DIR` can place disposable source/build caches
on a larger volume. Keep separate caches for different host operating systems.
The [validation record](docs/validation.md) distinguishes automated checks,
build-specific device observations and remaining release qualification.

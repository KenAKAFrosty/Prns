# Prns applications

This workspace owns the Prns app, its native composition and reusable application
services. It consumes public core APIs; the core does not depend on this tree.

## Start here

- [App behavior and routes](prns/app/README.md)
- [iOS development](docs/ios.md) and [Android development](docs/android.md)
- [Current validation and limits](docs/validation.md)
- [Implementation roadmap](docs/roadmap.md)

This guide owns setup and workspace commands. The binding and platform guides
own their narrower boundaries; dated checkpoints preserve historical evidence,
not current setup instructions. The original scratch plans are design history.

## Ownership

| Path | Responsibility |
| --- | --- |
| `prns/app` | Expo screens, navigation and application UI |
| `prns/native-composition/src` | Rust application policy, storage, command admission and the process-owned node |
| `prns/native-composition/bindings` | Generated JavaScript interface and the single shared Rust image packaged for each mobile target |
| `sdk/expo` | SDK admission plus native storage, lifecycle, permissions and Bluetooth integration; generated Swift/Kotlin views use the same Rust image |
| `services` | Reusable application services, including LXMF and its wire format |
| `tools/generated-bindings` | Generation, ownership checks and mobile build orchestration |
| `vendor/ubrn` | Pinned upstream runtime archives, patches and provenance |

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
npm --prefix applications ci --ignore-scripts --no-audit --no-fund
```

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

`verify` runs the portable Rust, generated-output, SDK, UI and web checks.
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

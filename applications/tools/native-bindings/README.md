# Application native image recipe

`image.json` selects `prns_chat_app` exactly once for the app's generated domain
namespaces and the staged general React Native SDK. Product API semantics use
explicit `appApiVersion: 3`, paired with `app_api_version()` in the image. Each
namespace's generated initialization also verifies UniFFI's ABI checksums.

`generate.py` uses `tools/uniffi/tooling.py` for pinned generation, output ownership
and mobile image packaging. Shared tooling normalizes whitespace and applies
the pinned [Kotlin checksum correction](../../../tools/uniffi/README.md#kotlin-checksum-correction)
before publishing generated files. The app does not patch domain declarations.
Its only assembly projections are the namespace exports/initialization and the canonical SDK host converter import.
The composition supplies its own entrypoints and omits the generator's aggregate
`index.ts`, which would initialize the SDK host binding namespace a second time.
This is binding initialization, not native host ownership: the app's service
runtime constructs and owns its host session. Domain namespace implementations
remain generated without modification.
Adding a domain method requires no change to this script or a private JS facade.

## Setup and generation

Start with the ordered [application quickstart](../../README.md#source-quickstart):
install and build `prns-js`, run `sdk:stage`, then install the application
workspace and its runtime patch. Staging requires the pinned Rust generator but
can run before the app's npm dependencies are installed. It checks tracked
bindings and materializes the selected SDK; it does not compile a mobile image.
Use `api:generate` when changing exports or updating generated files:

```sh
npm --prefix applications run api:generate
```

`bindings:ios` / `bindings:android` regenerate first and use the SDK's existing
packaging to build this same image. They do not establish device qualification.
The app's product bootstrap, platform permission UX and screen workflows are
separate consumers of these APIs.

Run from the repository root. Reuse one build target while keeping pinned
third-party generator sources in a separate cache:

```sh
CARGO_TARGET_DIR="$PWD/target" \
PRNS_UBRN_TARGET_DIR="$PWD/target" \
PRNS_UBRN_CACHE="$PWD/target/ubrn-tooling" \
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  npm --prefix applications run api:generate
```

When reusing this debug-disabled environment for the independent native Swift
suites pinned to Rust 1.96 on macOS, also set `CARGO_PROFILE_DEV_STRIP=none` and
`CARGO_PROFILE_TEST_STRIP=none`. That compiler’s debug-stripping pass can produce
[a misaligned Mach-O string table](https://github.com/rust-lang/rust/issues/157750).
The app’s pinned Rust 1.98.1 already includes the upstream fix and needs no such
override. Shared generation helpers preserve the caller’s profile settings.

## Binding validation

Run these checks after completing dependency installation in the quickstart:

```sh
npm --prefix applications run api:check
npm --prefix applications run api:test
npm --prefix applications run api:typecheck
npm --prefix applications run api:image:test
```

`api:check` rejects stale tracked bindings or staging. `api:typecheck` uses
installed TypeScript and runtime declarations; its `--node-modules` option can
inspect an existing dependency installation without modifying it.
`api:image:test` compiles all generated Swift namespaces and checks the real
native API version, product options, owner readiness and close. It requires
macOS and the Apple toolchain; it does not install the app.

## Unsigned Android qualification

`check_android.py` regenerates the one selected arm64 image, prebuilds only the
ignored Android project, compiles the complete SDK service and product module,
runs their maintained JUnit tests, and packages an **unsigned Debug APK**. A
qualification-only Gradle init file disables Debug signing and includes the SDK's
maintained tests from the repository; it does not change the product's build
configuration or publish test sources. This is a build/test gate, not Android
runtime, radio, release, or device evidence.

Prerequisites: Python 3.11+, JDK 21, the locked application npm dependencies with
`npm --prefix applications run patch:runtime` applied, built `prns-js` declarations, Android SDK platform 37.0, build-tools 37.0.0 and current
command-line tools, the NDK version in `tools/uniffi/react-native.json`, `cargo-ndk`,
and Rust target `aarch64-linux-android`. SDK licenses must already be accepted.
Select existing shared build/tooling caches explicitly:

```sh
ANDROID_HOME="$HOME/Library/Android/sdk" \
CARGO_TARGET_DIR="$PWD/target" \
PRNS_UBRN_TARGET_DIR="$PWD/target" \
PRNS_UBRN_CACHE="$PWD/target/ubrn-tooling" \
  python3 applications/tools/native-bindings/check_android.py --offline
```

Omit `--offline` when dependencies have not yet been cached. The script preserves
logs and its exact unsigned Gradle init file under
`applications/target/android-qualification/`, checks a 5 GiB free-space floor,
and writes `report.json` with the artifact SHA-256 and actual test counts. It
checks the packaged image's SONAME and 16 KiB alignment, the APK's unsigned state,
one arm64 PRNS image, private-storage backup setting, and the foreground service
manifest. `--inspect-only` repeats these artifact/report checks without a build;
it does not establish that existing artifacts match current source. The APK is
`applications/prns/app/android/app/build/outputs/apk/debug/app-debug-unsigned.apk`.
No install, signing, release packaging, or upload is performed.

# Generate the application bindings

From the repository root:

```sh
npm --prefix applications run api:generate
npm --prefix applications run api:check
```

Both commands use the source revision and patches recorded in
`applications/vendor/ubrn/source-lock.json`. The helper verifies that source,
builds the generator, builds the application metadata library, and generates
TypeScript, Swift, Kotlin, and contract identifiers from the same Rust API.
Generation also refreshes the canonical HostSnapshot adapters; checking rejects
any stale output without rewriting it. Do not edit generated files directly.

`PRNS_UBRN_CACHE` selects a disposable generator source/build cache; it defaults
to `applications/target/ubrn-tooling`. `CARGO_TARGET_DIR` selects the application's
Rust build directory. Put these on a large disk when needed. The cache is not a
source dependency and can be recreated from the committed source record.

Build the application's shared mobile image with:

```sh
npm --prefix applications run bindings:ios -- --sim-only --targets aarch64-apple-ios-sim
npm --prefix applications run bindings:android -- --targets arm64-v8a --release
```

The iOS command requires Xcode. The Android command requires the NDK recorded
in the source lock, `cargo-ndk`, and the requested Rust targets. It resolves
`ANDROID_NDK_HOME`, or the pinned installation under `ANDROID_HOME` /
`ANDROID_SDK_ROOT`, and rejects a different NDK. The normal native client build
helpers invoke this process before Expo prebuild and autolinking.

`prns/native-composition/bindings` owns the resulting dynamic framework or
Android shared libraries. Both native lifecycle calls and JSI use that image;
the Expo module must not package or link another copy of the Rust supervisor.
Mobile build outputs are ignored and rebuilt locally. Debug iOS builds enable
the existing bounded restoration diagnostic probe; release builds omit it.

The committed runtime npm archives are a separate dependency. Their source,
verification, rebuild, and upstream replacement procedure are documented in
[`../ubrn-vendor/README.md`](../ubrn-vendor/README.md).

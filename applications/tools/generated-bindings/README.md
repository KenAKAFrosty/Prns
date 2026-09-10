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
any stale or obsolete output without rewriting it. Do not edit generated files
directly.

`outputs.json` records the generator's complete ownership: its three generated
directories, the two generated files beside Rust source, and the application-owned
package scaffolding. Generation removes obsolete files only inside those declared
output directories. It rejects unowned output paths and symlinks before writing.
Keep handwritten files outside the generated directories. Native package metadata,
autolinking registration and entry points are maintained scaffolding; the source
generator does not rewrite them.

The SDK's record/enum export barrel is generated from the TypeScript bindings.
It excludes execution methods and Host transport records, so adding a Rust API
type does not require another handwritten export list or expose native-only
lifecycle functions through the SDK.

`PRNS_UBRN_CACHE` selects a disposable generator source/build cache; it defaults
to `applications/target/ubrn-tooling`. `CARGO_TARGET_DIR` selects the application's
Rust build directory. Put these on a large disk when needed. The cache is not a
source dependency and can be recreated from the committed source record. The
selected rustup binaries take precedence over system Rust installations. Building
the third-party generator uses its own compiler flags; application compilation
retains the caller's Rust flags and warning policy.

Build the application's shared mobile image with:

```sh
npm --prefix applications run bindings:ios -- --sim-only --targets aarch64-apple-ios-sim
npm --prefix applications run bindings:android -- --targets arm64-v8a --release
```

Both mobile commands first regenerate the typed sources, then build the shared
image; they never pair a newly built library with stale bindings. Use `api:check`
when the desired behavior is verification without rewriting source.

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

`npm --prefix applications run bindings:test` exercises output ownership, exact
integer/optional Host adapters and vendoring checks without building native code.

The committed runtime npm archives are a separate dependency. Their source,
verification, rebuild, and upstream replacement procedure are documented in
[`../ubrn-vendor/README.md`](../ubrn-vendor/README.md).

# Generate the application bindings

From the repository root:

```sh
npm --prefix applications run sdk:stage
npm --prefix applications ci --ignore-scripts --no-audit --no-fund
npm --prefix applications run api:check
# After changing Rust exports:
npm --prefix applications run api:generate
```

`sdk:stage` bootstraps a clean checkout before npm resolves the app's local SDK
dependency. It checks all tracked product bindings and writes only the disposable
`applications/target/react-native-sdk` package. `api:generate` explicitly refreshes
tracked product outputs and that stage; `api:check` verifies both without writes.

All commands use the source revision and patches recorded in
`vendor/ubrn/source-lock.json`. The helper verifies that source,
builds the generator, builds the application metadata library, and generates
TypeScript, Swift, Kotlin, and contract identifiers from the same Rust API.
The shared canonical pipeline generates the SDK host contract; this recipe
generates product bindings and refreshes the staged SDK namespace from the
same aggregate image. The canonical `prns-react-native` package always retains
its default image and generated outputs. Checking rejects stale or obsolete output without
rewriting it. Do not edit generated files directly.

`outputs.json` records the generator's complete ownership: its three generated
directories and the application-owned
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

`applications/target/react-native-sdk` owns the resulting dynamic framework or
Android shared libraries, selected by `bindings/host-provider.json`. The app,
platform facade, and product bindings all select that same `personal-rns-expo`
package. The maintained SDK sources are copied by the shared generator;
there is no app-maintained fork of its facade or platform glue. Product bindings
import that SDK namespace. Both native lifecycle calls and JSI use that image;
the Expo module must not package or link another copy of the Rust supervisor.
Mobile build outputs are ignored and rebuilt locally. Debug iOS builds enable
the existing bounded restoration diagnostic probe; release builds omit it.

`npm --prefix applications run bindings:test` exercises output ownership, exact
integer/optional Host adapters and vendoring checks without building native code.

The committed runtime npm archives are a separate dependency. Their source,
verification, rebuild, and upstream replacement procedure are documented in
[`tools/ubrn-vendor`](../../../tools/ubrn-vendor/README.md).

The generic build/generation/ownership implementation lives in
[`tools/uniffi`](../../../tools/uniffi/README.md). This directory contains only
the application's recipe, product export/fingerprint projection and debug
build policy. The general SDK supplies its own `BindingRecipe` to the shared
implementation and never imports application code.

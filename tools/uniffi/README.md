# Shared UniFFI build and generation tooling

`tooling.py` is independent of application contracts and directories. SDKs and
native compositions supply a `BindingRecipe` with their Cargo metadata manifest,
package, native image name, features, UniFFI configuration, and generated
selected TypeScript/Swift/Kotlin destinations. `generated_files(recipe, cli, env)` builds
one selected image and returns the generated sources without writing their
permanent destinations. Additional project projections remain caller-owned.

Each language is selected by its destination; omitted destinations default to
`None`. A Swift-only or Kotlin-only recipe passes `cli=None` and uses the image's
ordinary UniFFI bindgen binary. It neither imports React Native preparation nor
invokes that generator, and needs no Node/Expo installation. Native-only consumers
can use `tooling.py` without `react_native.py` or `react-native.json` in their
source tree. Include the image's bindgen feature in that recipe's `features`.
TypeScript-only generation does not build the native CLI.
An empty language selection is rejected before compilation.

```python
recipe = BindingRecipe(
    manifest=image_manifest, package="my-native-image", library_name="my_native",
    crate_dir=image_directory, swift_dir=output_directory,
    features=("uniffi-bindgen",),
)
files = generated_files(recipe, None, environment(target_directory))
```

Use `environment(target_dir)` to select the consumer build environment.

Before any project pre-generation writes, validate the complete output scopes
with `existing_outputs(*ownership(root, manifest))`. After collecting all
artifacts, call `synchronize_outputs(files, check, root, manifest)`. Ownership
manifest version 1 declares generated directories, individual generated files,
and maintained scaffolding. Checks reject path escapes, symlinks, overlapping
ownership, missing output families, stale files, and obsolete outputs. Normal
generation deletes only obsolete generated files, preserving scaffolding.

Run the focused shared checks from the repository root:

```sh
python3 -m unittest discover -s tools/uniffi -p 'test_*.py'
```

Native generation normally omits the CLI `--config` argument. UniFFI 0.31's
current CLI treats it as a complete replacement for every namespace config,
which collapses package/module names in an aggregate image. Each crate supplies
its own `uniffi.toml`; the selected library basename supplies Kotlin's image
name. Applications import the SDK's generated converter namespace through a
small generated re-export, instead of storing another converter implementation.

## Swift consumers

`compile_swift_consumer(...)` compiles the selected generated sources and C module maps
into one Swift library (including cross-namespace types), then compiles a separate
caller-selected consumer against it and a real native image on macOS. Supply
explicit generated-source and module-map lists, module-name, native-image,
fixture and output-directory paths plus the selected environment. The helper
returns the executable; the caller owns execution arguments, timeout, assertions
and generated metadata inventory checks. It performs no generation or RN setup.
Both Swift compilation steps keep their shared module cache under
`CARGO_TARGET_DIR/swift-module-cache` (or the supplied output directory when no
Cargo target is selected), so qualification does not depend on writing a cache
in the user's home directory.

## Kotlin checksum correction

Kotlin checksum comparisons use a strict shared backport for **UniFFI 0.31.2**.
That version intentionally returns Rust `u8`/`u16` through JNA's `Int` carrier,
but its checksum template compares the unspecified upper 16 bits too. This
fails on Android ARM64 when a valid `u16` checksum is sign-extended. Upstream
[PR #2935](https://github.com/mozilla/uniffi-rs/pull/2935), commit
`965691be2dd732853d082b991f3720b1181ad187`, fixes the comparison in
[the 0.32.1 template](https://github.com/mozilla/uniffi-rs/blob/v0.32.1/uniffi_bindgen/src/bindings/kotlin/templates/NamespaceLibraryTemplate.kt).

`generated_files` verifies the native generator's reported version, then
`normalize_kotlin_checksums` recognizes the complete 0.31.2 checksum function and
its matching `Int` declarations. Each call is narrowed with
`.toUShort().toInt()`, the same conversion as the generated `FfiConverterUShort`
(the converter itself is absent in namespaces without public `u16` values).
Every expected value, mismatch exception and contract-version check is preserved.
Missing checks, unexpected template content or a different generator version
fail generation before any owned output is written. Empty checksum inventories
are allowed. Review and remove this backport when upgrading the shared generator;
0.32 also changes configuration, constructor and byte-buffer APIs, so adopting
that wider upgrade is separate from this one-comparison correction.

Native-only and RN-selected recipes both use this path; no Cargo-cache mutation,
new dependency fork, or product-specific patch is involved. Android device qualification of a staged image remains the embedding application's responsibility.

# Shared UniFFI build and generation tooling

`tooling.py` is independent of application contracts and directories. SDKs and
native compositions supply a `BindingRecipe` with their Cargo metadata manifest,
package, native image name, features, UniFFI configuration, and generated
TypeScript/Swift/Kotlin destinations. `generated_files(recipe, cli, env)` builds
one selected image and returns the generated sources without writing their
permanent destinations. Additional project projections remain caller-owned.

Use `environment(target_dir)` to select the consumer build environment and
`generator(cache, env)` to obtain the one pinned ubrn generator. The generator
uses [`../ubrn-vendor`](../ubrn-vendor/README.md), with its single lock, patches,
and runtime archives under [`../../vendor/ubrn`](../../vendor/ubrn/README.md).
Consumer compiler flags do not leak into third-party generator compilation.

Before any project pre-generation writes, validate the complete output scopes
with `existing_outputs(*ownership(root, manifest))`. After collecting all
artifacts, call `synchronize_outputs(files, check, root, manifest)`. Ownership
manifest version 1 declares generated directories, individual generated files,
and maintained scaffolding. Checks reject path escapes, symlinks, overlapping
ownership, missing output families, stale files, and obsolete outputs. Normal
generation deletes only obsolete generated files, preserving scaffolding.

`build_mobile(cli, env, args, config)` validates the Android NDK against the same
source lock and builds the image selected by the caller's ubrn configuration.
The caller must generate current bindings before invoking it. Product debug
features and lifecycle policy belong to the caller, not this module.

Run the focused shared checks from the repository root:

```sh
python3 -m unittest discover -s tools/uniffi -p 'test_*.py'
python3 tools/ubrn-vendor/vendor.py check
python3 -m unittest discover -s tools/ubrn-vendor -p 'test_*.py'
```

The standalone SDK entry point is
`prns-react-native/tools/generate.py`. Native compositions can use this same
module with their own recipe and product projections, without adding a dependency
from the SDK back to the composition.

Native generation normally omits the CLI `--config` argument. UniFFI 0.31's
current CLI treats it as a complete replacement for every namespace config,
which collapses package/module names in an aggregate image. Each crate supplies
its own `uniffi.toml`; the selected library basename supplies Kotlin's image
name. Applications import the SDK's generated converter namespace through a
small generated re-export, instead of storing another converter implementation.

`vendor/ubrn/generator-patches.json` records emitter-only corrections applied by
the same source preparation routine. The first correction marks generated error
subclasses' `instanceOf` methods as `override`, allowing consumers to retain
`noImplicitOverride`. These patches cannot modify runtime sources. The runtime
source lock, archive hashes, and original runtime build receipt remain separate
and unchanged; no new runtime build is implied by a generator correction.

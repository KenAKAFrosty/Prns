# Pinned UniFFI runtime packages

The app consumes two generated npm archives from `applications/vendor/ubrn/packages`:
`@ubjs/core` and `@ubjs/react-native`. Their names and versions match upstream;
their exact bytes are selected by local archive paths and lockfile integrity.
They are built from the commit and ordered patches in
`applications/vendor/ubrn/source-lock.json`, not from a moving branch or a
workstation checkout. The archives contain upstream's license notice and a copy
of that source record. App bindings remain generated from the app's Rust API.

This temporary distribution keeps ordinary `npm ci --ignore-scripts` usable on
Linux and macOS. Installing JavaScript dependencies does not download an
unreviewed Git head or build five mobile Rust targets. The shared runtime's
native images are separate from the app-owned Rust image: the app still builds
one `prns_app` shared library per target for both generated JS and native bindings.

Verify the committed archives without Xcode, Android tooling, or npm packages:

```sh
python3 applications/tools/ubrn-vendor/vendor.py check
python3 -m unittest discover -s applications/tools/ubrn-vendor -p 'test_*.py'
```

Rebuild on macOS with the versions recorded in `source-lock.json`. Install its
five Rust targets in the selected rustup toolchain and install the recorded
`cargo-ndk`. Set `ANDROID_NDK_HOME` to that NDK and choose a disposable cache
directory with enough space:

```sh
python3 applications/tools/ubrn-vendor/vendor.py build --cache-dir /path/to/large/cache
```

`--rust-toolchain` selects a rustup name; it must resolve to the recorded Rust
version, and its actual Rust binaries take precedence over system installations.
Apple tools are selected through `xcrun`, ahead of environment-provided
compilers. The helper remaps source paths, preserves Android's soname/alignment
flags, builds all five slices, runs upstream's packaging/completeness checks,
and records archive hashes and the patched source tree in `receipt.json`.
`--native-only` builds and caches the slices without sealing packages. A change
limited to the C++ shim can reuse those slices after their hashes and all Rust
build inputs are checked.

The native compiler output need not be byte-identical across unrelated build
hosts. `npm ci` installs the exact reviewed archives; it never silently rebuilds
them. A deliberate rebuild updates the receipt and archives together, followed
by review of their source record and native tests. Cache contents are disposable.

Receipt format 2 distinguishes the build recipe from the verifier:

- `buildScriptSha256` identifies the exact helper that produced the archives.
  `buildScriptRevision`, when present, locates that same helper in repository
  history. A rebuild from an edited or detached helper records its hash without
  inventing a matching Git revision.
- `verificationScriptSha256` identifies the reviewed current helper used by
  `check`. A helper-only correction can update this field while retaining the
  actual historical build hash, source record, archive hashes and native evidence.
  Missing or changed verifier hashes fail verification until reviewed.

The current archives still come from the recipe recorded at `842271720`.
The toolchain-selection cleanup updated only their verifier record; it did not
rebuild the archives or claim new native qualification. Archive checking needs
neither Git history nor a native compiler. It still verifies both archive bytes,
patch/source hashes, embedded source provenance and required package contents.

To update upstream or remove an accepted patch, update the source revision and
ordered patch hashes, rebuild, review the package diff and receipt, update the
app's npm lockfile, and rerun the detached-consumer and native lifecycle gates.
Do not edit files inside the tarballs or generated bindings. Once an upstream
release includes the required fixes, replace the local selections with exact
registry versions and remove this temporary vendor distribution.

Keep this helper and its source record inside `applications/`: the detached
consumer gate exports only that subtree. Its lock refresh must leave the runtime
archive selections and integrity unchanged. Generated-code verification should
use the same source revision and patches as this distribution.

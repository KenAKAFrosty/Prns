# Detached application mobility smoke

This check exports only the tracked `applications/` subtree, resolves every
base Prns dependency from the exact revision in
`release/compatibility.json`, and runs the generated-binding, Rust, npm, Expo
web-export, and LXMF gates in a temporary workspace. It rejects symlinks,
resolver overrides, unreviewed escaping dependencies, stale compatibility
metadata, and any change to tracked base Prns files in the source checkout.
The compatibility record names the complete Rust source-package set and the
expected JavaScript tarball digest. Cargo may replace path-package source fields
with the exact Git source, but every package, version, checksum, and dependency
edge in the committed lock must remain unchanged. The npm lock is held to the
same rule except for the recorded tarball and its exact production dependencies.
The committed `vendor/ubrn/` runtime archives remain inside the export with their
original npm selections and integrity. Their receipt is checked before install.
The bindings workspace resolves its `personal-rns` peer from the same detached
artifact as the app and SDK.

The normal `verify` gate invokes `api:check`, which rebuilds the pinned generator
from the exported source lock and patches and compares all TypeScript, Swift,
Kotlin and canonical Host outputs. It does not rewrite stale generated files.

Run it from a Prns checkout with:

```sh
npm --prefix applications run mobility:verify
```

Set `PRNS_UBRN_CACHE` to reuse a disposable generator cache, and pass
`-- --keep-workspace /path/to/new-workspace` to retain the complete detached
workspace on a chosen volume. The generator still verifies the app-local source
pin and patches before reusing cached source; this option cannot select a
different dependency version. With neither option, caches and builds stay in
the temporary detached workspace.

The runner requires the recorded Node and npm versions and the recorded minimum
Python and Rust versions. CI provisions those tools explicitly and runs this
suite as a release-critical lane.

By default the wrapper uses the current checkout as a local `file:` Git URL.
Set `PRNS_MOBILITY_GIT_URL` to exercise the same recorded revision through a
remote repository after that commit is available there. The local run proves
source mobility and exact revision selection; it does not claim remote
reachability, clean Expo autolinking, native platform packaging, or a physical
device journey.

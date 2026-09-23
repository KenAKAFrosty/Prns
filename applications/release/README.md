# Compatibility and current-source evidence

`compatibility.json` is the recorded release contract. Its Prns commit, Host
schema digest, Rust package inventory, and JavaScript artifact digest are
historical facts. The React Native SDK migration does not invent a replacement
commit or update those digests to describe uncommitted files.

`npm run mobility:verify` remains the recorded-revision gate. It requires
committed application and shared binding inputs, checks out the exact recorded
Prns commit, and admits only reviewed dependency-source changes. The exported
application now includes the sibling `prns-react-native` package and shared
`tools/uniffi`, `tools/ubrn-vendor`, and `vendor/ubrn` paths. The SDK Android JNI
leaf is an explicit exported package; its core dependencies remain subject to
the recorded Rust package inventory. Canonical SDK conversion metadata must
exist in the pinned core checkout. A missing package or metadata file fails the
gate; local checkout contents never substitute for that commit.

The old recorded revision predates the new general SDK. A release promotion
must therefore record a real containing commit, review the resulting complete
Rust dependency inventory (including the host facade/native crates), and record
the actual built JavaScript artifact digest. Source migration or local tests
alone do not perform that promotion.

For work before release promotion, use the separate current-source gate:

```sh
npm --prefix applications run mobility:verify -- --working-tree --snapshot-only
npm --prefix applications run mobility:verify -- --working-tree --keep-workspace /tmp/prns-current-check
```

`working-tree-qualification.json` reviews the source roots and dependency
entrypoints for that mode. It includes the SDK, host facade/native/default
image, Android JNI leaf, and their complete local Rust dependency closure. It
includes tracked modifications and nonignored new source, excludes build caches,
and rejects symlinks, resolver overrides, and escaping/unreviewed dependencies.
The receipt identifies every exported file by content hash and marks
`releaseQualified: false`. Concurrent edits during capture reject the snapshot.
The root Cargo workspace retains its inherited package, lint and profile policy;
only its member/default-member lists are projected to the reviewed dependency
closure. The receipt records both manifest hashes and every omitted member.

`--snapshot-only` validates export and dependency closure without compiling or
installing anything. The full current-source mode builds the current core
JavaScript package, packs core and SDK code into actual npm artifacts, installs
them outside the original checkout, preserves unrelated npm lock entries, and
runs the SDK's locked source typecheck/tests, generated binding checks, aggregate
foreign-object sharing, native tests, platform checks, app checks and LXMF interop.
It intentionally does not run the historical compatibility/artifact
version gates against those new artifacts. Its success marker is
`APPLICATION_WORKING_TREE_QUALIFICATION_OK`, distinct from the release marker.

This source gate does not attest mobile binaries or publication. The SDK's
`npm run test:consumer` separately checks its packed default native image in an
independent Expo consumer; device, iOS restoration, and app aggregate-image
qualification remain separately reported evidence.

# September 17: app dependency notices and publication

The user approved the narrow license-policy correction left open by the
[upstream refresh](2026-09-17-remote-control-refresh.md).

## Approved scope

Add exact `0.31.2` MPL-2.0 exceptions for `uniffi`, `uniffi_core`,
`uniffi_internal_macros`, `uniffi_macros`, `uniffi_meta` and `uniffi_pipeline`.
These are existing generated-binding dependencies, not new packages or upgrades.
Both general license allowlists remain unchanged. The notice tool selects their
MPL text by package name; the dependency policy enforces the exact versions.
The vendored React Native runtime's license and source/patch records are intact.

The canonical notice generator and release dependency audit now include the
app's Android and iOS native composition, with the `android` and `apple` features
respectively, in addition to default features. Existing product graphs retain
their previous selections. Do not use an all-features union: that would add
binding-generator tooling that is not part of these app runtime builds.

## Focused verification

- Both actual locked platform dependency graphs pass advisories, bans,
  licenses and source checks.
- All 13 notice-tool tests pass, including platform feature forwarding and
  execution of the audit command construction under macOS Bash 3.2. That shell
  regression is explicitly limited to POSIX hosts with Bash; the other tests
  remain portable.
- General allowlist parity and exact six-package exception scope are verified.
- Canonical notices were regenerated from freshly fetched, locked sources and
  pass the input-fingerprint check. All previous license texts are unchanged;
  the bundle records both app platform selections and all six UniFFI packages.
- Application code, dependency versions and lockfiles are unchanged by this
  correction. The core compatibility pin remains `6fe326c02`.

## Publication and exact-build evidence

Implementation `85cbd37544c09147abf169e5992e08441a6fe4da` was published to
`prns-app` after the normal pre-push hook passed, using the previously verified
remote head as an exact lease. Both the Git remote and draft PR #197 reported
that implementation head. Upstream remained `c4fd54dfd`. The subsequent checkpoint
commit is documentation only; the results below belong to `85cbd3754`.

- Fresh detached qualification passed against core `6fe326c02`: 136 native unit
  tests, four integration tests, 49 SDK tests, 223 UI tests, generated-output
  checks, Expo Doctor 21/21 and web export. Live Python LXMF interoperability
  passed in both directions. This detached run used a local Git source URL.
- Separately, a fresh empty repository fetched the exact core pin from the
  declared `https://github.com/KenAKAFrosty/Prns.git` URL after publication. Its
  package, Bluetooth and Host-contract compatibility checks passed with the
  fetched revision as HEAD; no local object store was reused.
- Fresh Swift startup-queue, protected-data recovery, restoration diagnostics
  and release-symbol checks passed.
- The normal publication hook passed all 24 host workspace checks and its
  selected core, integration, browser and dependency-policy checks. All 14
  configured firmware resource reports succeeded, as did the quick Miri suite
  and all three target-ISA suites.
- All 14 resource reports name `85cbd3754`. They are classified as
  `working-tree`, not `clean-commit`, because existing untracked IDE/vendor files
  were preserved; tracked files were unchanged during the run. All four Miri/ISA
  result records name the same commit and record a clean tracked tree.
- T-Echo S140 v7 has 4,396 bytes of flash headroom. T096 has only 80 bytes of
  known RAM headroom after the unchanged reservations; the passing build is not
  a claim of comfortable runtime margin. Nordic toolchains differ from the
  committed resource baseline, so its deltas are not same-toolchain comparisons.

Hosted checks were queued when the implementation head was verified; local
passes do not establish remote CI success. This correction does not establish
new physical-device evidence or complete production distribution and
license-notice presentation in the apps. No phone was installed or board flashed.

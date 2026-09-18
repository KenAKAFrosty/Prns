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

Normal publication and detached qualification are separate from these focused
results. This correction does not establish new physical-device evidence or
complete production distribution and license-notice presentation in the apps.

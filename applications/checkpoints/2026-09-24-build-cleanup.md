# SDK and app build cleanup

This follow-up to [SDK adoption](2026-09-24-sdk-adoption.md) closes build coverage
and documentation gaps before further app features. The shared-host, SDK and app
ownership boundaries are unchanged.

## Changes

- Release readiness installs the pinned SDK compiler and npm toolchain, builds
  the core JavaScript contract and installs SDK dependencies before its SDK
  suites. LXMF qualification installs its embedded target on the release
  compiler; detached app qualification installs the recorded stable toolchain,
  components, target and npm version. Other release suites retain their existing
  compiler selection.
- PR CI builds the actual app aggregate on Android and iOS, in addition to the
  independent SDK. The Android Release APK and unsigned arm64 iOS simulator
  Release app include JavaScript and exactly one PRNS image. Both matrix entries
  are required by the final CI gate.
- `native:ios:build` reuses the existing helper with `--build-only`, including
  Release Rust generation, metadata checks and explicit simulator architecture.
  It requires no signing, simulator startup or installation. Existing interactive
  simulator arguments are covered by a macOS Bash regression check.
- The SDK source quickstart is sequential, including the WebAssembly target and
  matching CLI required for the browser example. Current app docs distinguish default
  SDK sessions from native app-owned sessions, and completed bounded checks from
  remaining release and background qualification. Historical chooser tasks and
  publication chronologies no longer appear as active roadmap work.
- Obsolete aggregate libraries were removed from the original checkout's
  canonical SDK output directories; the app uses its own staged package.

## Local validation

The complete Android Release build and unsigned iOS simulator Release build
passed, including bundled JavaScript, image selection and platform metadata.
Android additionally passed its packaged minimum-API and 16 KiB alignment checks.
Strict app and standalone SDK generation checks passed afterward without changes
to committed product or SDK bindings.

| Artifact | SHA-256 |
| --- | --- |
| Android APK | `44c1750471aa3557b08832a1bdc921c949aa4f5e2f815c80ff29bcdbc38196de` |
| iOS simulator executable | `b3c0cdd1a90d46869b829f29a88b35b701b9d294e55bdb19de67000554a0b8aa` |
| iOS simulator PRNS framework | `ab3ea88afc798edfb34ac0f39ecc3f06d43b84847bb1377b4747f2726e88f1c4` |

The clean SDK dependency setup, strict TypeScript, 21 SDK JavaScript tests and
14 SDK Python tests passed. All 14 workflow regression tests, actionlint, the validation
registry, shared generation/vendor tests, app dependency boundary, Apple
lifecycle/release-symbol tests and documentation links also passed.

These are local build and test results. Hosted CI, detached aggregate iOS
compilation, physical lifecycle acceptance and production distribution retain
their separate scopes. The recorded app release revision is still historical;
promotion waits for the SDK to land and for reviewed matching artifacts.

## Separate upstream CI repair

The missing board baseline was regenerated through the official qualification
tools from clean upstream commit `58db1c0c4350e950c77ff9cabfee9d1cda58df1e`.
All 15 firmware resource targets, full Miri under both borrow models, all three
ISA suites and the ESP32-S3 startup pilot passed before any source edits. The
accepted matrix contains 15 targets and nine capabilities. Its SHA-256 is
`48aae5f49f1d47bfccbf742ab7bd5486d57f2a5da505c66aef68ee59b0b6f750`.

The repair adds the missing board's placeholder CLI/web roster entries and
replaces stale numeric test fixtures with canonical target identities and
registry-derived counts. All 48 assurance and 13 roster tests pass, along with
the assurance crate's formatting and Clippy checks. Completeness requirements
remain enforced; placeholder roster entries make no hardware-acceptance claim.

This repair is preserved independently on `codex/upstream-assurance-cleanup` and
included in the local SDK/app cleanup. Its evidence qualifies the recorded
upstream commit, not new SDK or app firmware behavior.

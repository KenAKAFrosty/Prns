# September 17: upstream remote-control integration

## Source and scope

Rebase the app and remaining contribution branches on upstream
`79050535f7323043fb5d9fac829e5267aabcbfad`. This includes PR #232's expanded
remote control, subsequent Wi-Fi teardown and split-resource identity fixes,
feature-gated validation corrections and clean-checkout notice fingerprinting.

The app integration retains the upstream board command executor and transactional
authorization persistence alongside the existing pairing UI and mobile transport
work. Core snapshot `6a3a19d5cc77845663482d08bb9925ce67ac898c` adds regressions
ensuring capability growth does not widen the local pairing grant. It remains an
Operator grant for Describe/AnnounceSelf intersected with board capabilities.

Application compatibility changes use the new explicit host-control state and
authority APIs. The generated contract names all 28 request kinds, and pairing
confirmation displays Operator or Administrator access. Regenerate the bindings
from Rust; there is no handwritten native bridge implementation added here.

The controlled persistence-failure test now verifies the upstream fail-closed
behavior: when both the authorization write and durable rollback fail, the
runtime stops, exposes a persistence failure, retains no paired target, and
retires the peer Link. The old expectation that it remain Running was no longer
correct. No production persistence failure is suppressed to preserve that test.

## Completed app checks

- Native composition: 136 unit tests and four integration tests pass, including
  real TCP pairing, persistence failure, cancellation and route-readiness cases.
- UI: 223 tests pass across 27 suites; formatting, lint, type checks, route and
  configuration checks, Expo dependency checks, Doctor 21/21 and web export pass.
- SDK: all 49 tests pass; five compatibility and 12 detached-tool unit tests pass.
- Three board pairing regressions pass for capability intersection, refusal when
  no initial permission is supported, and no new control/admin escalation.
- Generated-output tooling/provenance tests and explicit Swift startup queue,
  lifecycle, restoration diagnostics and release-symbol checks pass.
- The recorded exact core revision and unchanged Host schema fingerprint pass
  the application compatibility check.

Xcode SDK access was restored during this run. The initial Doctor failure came
from an obsolete Ruby/CocoaPods executable on the shell path; the already
installed CocoaPods 1.16.2 passed without changing global tools or app dependencies.
Historical cached Rust build-script warnings about the earlier license block do
not replace the subsequent successful SDK lookup and Swift test results.

Normal publication gates and clean detached verification are recorded separately
as they finish. Focused results above are not a claim that every remote CI job or
every contribution branch passes its entire matrix.

## Publication accounting

PR #214 was closed as fully superseded: all of its notices are in trunk. PRs #198
and #213 are already merged. PRs #216, #221 and #224 already contain the exact
current trunk and retain their existing history. PRs #199 and #203 retain only
useful test improvements after their production fixes landed upstream. Other
contributions remain separate, with their dependencies and validation recorded
in the refresh report; force pushes use the captured remote heads as leases.

## Limits and next work

No phone was installed or board flashed for this refresh. September 15 Android
and older iOS physical evidence remain tied to their original binaries. The
observed E290 menu freeze remains unresolved. Earlier firmware headroom values
do not qualify this rebase.

The [expanded remote-control plan](../docs/remote-control-expansion.md) is the next
product sequence, not an implemented feature list. It covers read-only inventory,
ordinary controls, disruptive actions, transactional Wi-Fi and administrator
access, including the narrow upstream API gaps and separate mobile qualification.

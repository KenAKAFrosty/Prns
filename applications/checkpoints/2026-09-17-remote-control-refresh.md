# September 17: upstream remote-control integration

## Source and scope

Rebase the app and remaining contribution branches on upstream
`c4fd54dfd7a83048fc2cf5096a09fec6fa17a0ec`. This includes PR #232's expanded
remote control, subsequent Wi-Fi teardown and split-resource identity fixes,
feature-gated validation corrections and clean-checkout notice fingerprinting.
During publication, upstream also merged our remaining-window, local-Link-close
and disconnected-packet corrections (#216, #221 and #224). The remaining open
branches were refreshed again to include those merges.

The app integration retains the upstream board command executor and transactional
authorization persistence alongside the existing pairing UI and mobile transport
work. Core snapshot `6fe326c02e7c6b8503bf9bc2397320e791266ae7` includes regressions
ensuring capability growth does not widen the local pairing grant, and updates
the shared-snapshot, runtime-event and diagnostic test fixtures for the new API. It remains an
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
- All four runtime-event regressions pass with the fallible pairing-offer API.
- All 131 Embassy library tests pass after removing the duplicate pending-failure
  payload; queue ordering, backpressure and retry-deadline coverage are retained.
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

The first detached attempt passed its native checks but timed out one existing
UI test under concurrent build load. Concurrent core-fixture edits in the source
checkout also invalidated its unchanged-source guard. That run is not counted as
successful detached qualification; repeat against a stable committed checkout.

The stable retry passed the complete detached gate at app `ea9b7c4a5` / core
`abf32db96`, including native composition, UI/SDK, generated bindings and live
Python LXMF interoperability. A subsequent repository-wide host check passed 23
of 24 workspaces and found one additional runtime-event test fixture using the
old infallible pairing-offer API. The corrected fixture passes all four event
tests; the pin now includes that test-only correction. Publication and detached
checks are repeated against the final pin rather than attributing the earlier
result to a different revision.

That repeat also passed at app `1fe2ec0ae` / core `74ed674be`. The normal app
publication gate then passed all 24 host workspaces and reached the firmware
matrix. T-Echo S140 v7 linked with 4,524 bytes of flash headroom, but T096 exceeded
its unchanged static-RAM/stack-reserve limit by 24 bytes. Trunk itself leaves no
spare bytes in that T096 accounting. PR #215 alone did not fix the RAM increase.

The persistence scheduling correction keeps failure reports in the existing
one-entry channel until progress observes them, removing a second resident
payload without changing retry deadlines or authorization transactions. The
isolated pairing composition saves 104 bytes and links T096 with 80 bytes beyond
the unchanged stack reservation. That small link-time margin is not a runtime
stack qualification. The app pin includes the correction; its full publication
and detached checks are repeated separately.

The next detached verification passed at app `c58868a8f` / core `457a2ff0e`,
including live Python LXMF interoperability. The normal publication run passed
all 24 host workspaces, all 14 firmware profiles, Miri and three-ISA execution,
integration and browser checks, but then failed the dependency license policy:
six existing UniFFI 0.31.2 crates require narrow MPL-2.0 exceptions. That run did
not publish the app. No exception or gate bypass is assumed here.

That exact run linked T-Echo S140 v7 with 4,396 bytes of flash headroom and
T096 with 80 bytes beyond the unchanged RAM stack reservation. The source was
tracked-clean but had pre-existing untracked IDE/vendor files; its reports label
it a working tree, not a clean commit. These are link-time measurements, not
physical-device or runtime-stack qualification.

The second rebase produced app `df021e163` / core `6fe326c02`. Before the pin and
documentation update, their complete tracked trees matched `c58868a8f` and
`457a2ff0e` respectively: the newly merged changes were already integrated in
the app. The new revision needs its own publication result; the earlier failure
is not reported as a successful publishing gate.

Detached verification passed again at app `173a36df0889b0c205b1710f23be9b6d3679d555`
with core `6fe326c02e7c6b8503bf9bc2397320e791266ae7`: generated bindings, native
composition, 223 UI tests, 49 SDK tests, web export and live Python LXMF exchange
all passed. This uses the local Git source and establishes exact-revision source
mobility, not remote availability or native-device acceptance. App publication
remains blocked on the narrow UniFFI license exceptions pending approval.

License-only checks were repeated at app `e9e8a54c1`, independently selecting
the locked Apple/iOS and Android release feature graphs. Both reject exactly
the same six existing UniFFI 0.31.2 packages. No source, lockfile, general license
allowlist or publishing hook was changed to bypass that result.

## Publication accounting

PR #214 was closed as fully superseded: all of its notices are in trunk. PRs #198
and #213 are already merged. PRs #216, #221 and #224 were merged while this
refresh was running. PRs #199 and #203 retain only
useful test improvements after their production fixes landed upstream. Other
contributions remain separate, with their dependencies and validation recorded
in the refresh report; force pushes use the captured remote heads as leases.

The core publication pass is complete. All 22 remaining contributions were
force-pushed onto `c4fd54dfd`, with titles/bodies updated and independently read
back: #199–#212, #215, #217–#220, #222, #223 and #225. Each passed its normal
selected publishing checks, including all 14 configured firmware profiles.
Each preserved matrix and four Miri/target-ISA results name the exact clean
published commit. These are branch-specific results, not app-device evidence.

The final audit at 21:39 UTC matched prepared heads, original local branch
names, the actual Git remote and GitHub. All open PRs retain their existing
draft states and `trunk` base. Upstream remained `c4fd54dfd`. All 22 still had
pending hosted checks, with no new-head failure reported at that snapshot.
The separate casework smoke limitation remains disclosed in #206; older hosted
timing/CodeQL observations remain attributed to their original heads.

App #197 is deliberately not included in that completed-push count. Its remote
head remains `f3755a977`; the rebased source, compatibility changes and expanded
control plan are committed locally, awaiting the scoped license-policy decision.
Fresh detached qualification does not imply that its new core pin is available
from the declared remote repository. Verify that separately after publication.

## Limits and next work

No phone was installed or board flashed for this refresh. September 15 Android
and older iOS physical evidence remain tied to their original binaries. The
observed E290 menu freeze remains unresolved. Earlier firmware headroom values
do not qualify this rebase.

The [expanded remote-control plan](../docs/remote-control-expansion.md) is the next
product sequence, not an implemented feature list. It covers read-only inventory,
ordinary controls, disruptive actions, transactional Wi-Fi and administrator
access, including the narrow upstream API gaps and separate mobile qualification.

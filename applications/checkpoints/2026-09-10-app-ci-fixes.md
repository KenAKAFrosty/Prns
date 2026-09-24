# App publication test fixes — September 10, 2026

The readiness review after the [five-PR refresh](2026-09-09-pr-refresh.md)
found two failures on the published app head that were still present locally.
Both are now corrected without changing production code, firmware limits,
validation order, generated bindings or the app's core pin `451e669da`.

| Fix | Separate local candidate | App integration |
| --- | --- | --- |
| Unix-only Host snapshot test import | `codex/fix-host-snapshot-windows-tests` at `bb58bd6c4e80edf31527e6b9f0e4f5bf828a258f` | `a5fee4de5b2acfb653ff825c1d95cff7645f35ba` |
| Bounded source-archive rejection fixture | `codex/fix-source-archive-custody-test` at `d4924478ee831e6866329e4cf4fa3d2fa322bbb7` | `7078e8f3646a7685f77661bcd516a25673369a4a` |

## Windows Host tests

[The Windows job](https://github.com/KenAKAFrosty/Prns/actions/runs/34352075399/job/102467545652)
rejected an unused `InterfaceHealth` import under warnings-as-errors. Snapshot
extraction had moved its cross-platform consumer, leaving only the Unix-specific
connection-wait helper. The import now lives inside that helper.

The isolated candidate stacks on the published #201 head `9a955424b`, not current
trunk, which does not contain the offending import. It should accompany a future
#201 update; the original local and published branch remain unchanged.

All 24 native Host tests, strict all-target Clippy and formatting passed in a
fresh, candidate-owned build directory. A real Windows all-target cross-check
was attempted after installing the Rust Windows standard library, but `ring`
could not compile without Windows SDK headers (`assert.h`). Explicit unwrapped
Clang produced the same missing-header limit. This is not a passing Windows
build or a reason to disable warnings; Windows CI must confirm it after publication.

## Release custody test

[The release-contract job](https://github.com/KenAKAFrosty/Prns/actions/runs/34352075399/job/102467545594)
expected source-embedding rejection, but the complete app repository archive
exceeded the ESP firmware region first. The unchanged test reproduced that
failure locally before the correction.

Only this test now uses a small, real Git repository containing the required
source files. It proves that the clean candidate passes full validation and
that the modified firmware still fits the real ESP region, then requires the
specific rejection for embedding the complete source archive. No check is
mocked or relaxed. Other fixture users retain the actual checkout and commit.
Existing source-identity, checksum, firmware-size and custody cases remain intact.

The focused regression and all 29 custody tests passed on the separate candidate;
the Python undefined-name check and independent source review also passed.
The candidate is based directly on checked trunk `1d4d4ba865` and can be proposed
independently. It keeps app growth from changing which guard this test exercises.

## Integrated verification and publication boundary

The integrated native Host passed all 24 tests and strict all-target Clippy on
`a5fee4de5`, using a separate, initially empty app build directory with actual
main-checkout source paths verified. The next commit changes only the Python
fixture. Compatibility, application-boundary, validation/tooling registries,
personal-path and whitespace checks also pass. The full release-contract command
completed successfully on `7078e8f36`: Python undefined-name checks, workflow,
feature and acceptance-document contracts, and all 297 tests passed. Those 297
include the 29 custody tests; they are not additional cases. Relative Markdown
targets were also checked. No full app, detached, firmware or publishing gate
was substituted by these focused results.

No push, PR edit, phone installation or board operation occurred. The app PR
remains a collaboration draft at remote `31532838b`, and upstream was unchanged.
The current app still needs its full publishing checks, including a fresh
firmware matrix, plus current-pin app/detached verification and updated PR text.
The historical 616-byte app firmware margin is not a measurement of these tests
or a waiver of that final gate. These fixes do not establish full remote CI,
CodeQL alert resolution or additional physical-device qualification.

Private evidence is under
`/Volumes/wavlink/dev/prns-app-acceptance-20260910/host-windows.dntnPr/` and
`/Volumes/wavlink/dev/prns-app-acceptance-20260910/release-custody.K4MFWW/`.
The local upstream report records candidate bases and proposed maintainer text.

# App publishing qualification — September 10, 2026

Continuation of the [two CI test fixes](2026-09-10-app-ci-fixes.md). The user
approved completing the final checks and updating collaboration draft #197.
The app source tested here is `c2c1631f02cad11c9129e7d49f6b4bda583ca8da`, with
core pin `451e669da33ec885f3c03cdb2ba26f8ba984caba`. Subsequent documentation-only
changes record these results; they are not new phone builds.

## App and extraction checks

A new qualification worktree and separate, initially empty root, app and
detached build caches prevent cross-checkout artifact reuse. Source paths,
the exported Git archive commit and the detached core checkout were verified.
Tracked generated output stayed unchanged.

| Check | Result |
| --- | --- |
| Full app verification | 136 native unit and four integration tests; 49 SDK and 221 UI tests; generated output, strict checks, Expo Doctor 21/21 and web export passed |
| Explicit iOS platform tests | Six startup-queue/admission cases, protected-data recovery, diagnostic parsing/sanitization and release-symbol checks passed |
| Clean detached application | The same app suites passed against exact core `451e669da`; pinned-Python LXMF interoperability passed over two Links with inbound content and outbound proof |

The detached JavaScript artifact retains SHA-256
`0b292b27e05d5c3f4e6f0d0861c1ef159059c62116ac983105f055eee5a71296`.
Its source was resolved through local Git. This validates extraction and exact
dependency selection, not remote availability or native phone packaging.
Repeated suites are separate runs of the same cases, not additional test counts.

## Repository and firmware checks

The unchanged pre-push hook passed all 24 host workspaces and all 12 selected
parity gates for the prospective app update. Core external-allocation checks
passed 1,959 tests, with three existing profiling ignores, plus two crypto and
two documentation tests. The full configured firmware matrix, strict lint
checks, browser smoke tests and reviewed unsafe-dependency inventory passed.
This is local publishing qualification, not a result for every GitHub CI job.

All fourteen configured firmware reports are successful. The current T-Echo
S140 v7 image uses 621,968 of 622,592 FLASH bytes, leaving 624 bytes. Statically
accounted RAM leaves 4,680 bytes after 138,680 static bytes and 69,632 reserved
bytes. This is a fresh measurement on the app source with rustc 1.98.1, not the
472/480-byte results from the separate upstream contributions. It updates the
earlier app's 616-byte measurement without attributing that small difference to
a particular source change. ESP image sizes may also depend on checkout paths.

The [earlier 1,984-byte overflow](2026-09-09-validation.md#constrained-nordic-firmware-failure-and-publication-exception)
remains historical evidence. No layout, capacity, feature budget, validation
policy or hook has been changed to pass the current checks. Firmware fit is not
physical runtime memory or optional-configuration qualification.

## Publication scope and CI limits

The intended update is an ordinary fast-forward of `origin/prns-app` from
`31532838b34bf48158eeef7b0f9b7f2a42706b84`, with the normal hook enabled. Preserve
#197's title, `trunk` base and draft state, and verify the resulting head and
description. Upstream was checked at `1d4d4ba8652875ee6fda835c633398d1bab3419a`.
Only the app PR is in this update's scope; the separate recovery, write-batch,
Nordic and test-fix candidates remain governed by their own reports.

The preceding [test-fix pass](2026-09-10-app-ci-fixes.md) passed 297 release-contract
tests and 24 native Host tests with strict Clippy. Its Windows cross-check
stopped at missing SDK headers before PRNS compilation. New Windows CI results
are required; a local macOS pass does not stand in for them.

The previous CodeQL aggregate had six annotations. Bounded source review found
four test-only IVs, a production IV buffer randomized before use, and UUID-length
diagnostic logging. No exploitable defect was established in those reports;
this is not a blanket review of vendor logging. Alerts were not dismissed and
security settings were not changed. A current-head scan and maintainer triage
remain distinct from local build validation.

No phone was installed or board flashed. Earlier physical evidence remains tied
to its exact builds; fresh pairing, broader background/locked-device behavior,
newer Android versions and production distribution remain unqualified. Browser
and Tauri runtime providers remain unimplemented.

Private logs, exact commands, original/proposed PR text and source-provenance
records are retained under
`/Volumes/wavlink/dev/prns-app-acceptance-20260910/app-publication.8tDC7i/`.

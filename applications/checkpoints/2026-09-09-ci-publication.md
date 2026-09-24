# Independent CI corrections published — September 9, 2026

The later [five-PR refresh](2026-09-09-pr-refresh.md) records the separately
approved USB/iOS head updates. The untouched-head statements below describe this
earlier publication checkpoint.

Continuation of the [correction and integration record](2026-09-09-ci-corrections.md).
The two independent contributions are now published. Upstream trunk was checked
again before opening them and remained at
`1d4d4ba8652875ee6fda835c633398d1bab3419a`; no rebase was necessary.

| PR | Branch | Published commit |
| --- | --- | --- |
| [#213: Preserve split-resource identity after a segment completes](https://github.com/KenAKAFrosty/Prns/pull/213) | `codex/fix-live-resource-continuations` | `19106615d7d80a6d021359c5aa02a6bad889edb9` |
| [#214: Refresh notices for embedded tooling dependencies](https://github.com/KenAKAFrosty/Prns/pull/214) | `codex/fix-third-party-notices` | `e675bc95ae9466cb7d6f17e59f0fe236afe9fc30` |

Both are open, non-draft PRs directly against `trunk`, with maintainer edits
enabled. Their posted bodies explain the app-development motivation and keep
app-only changes out of these contributions. Remote readback matched both exact
commits. GitHub checks had started but were incomplete at 00:09 UTC on September
10 (September 9 locally); this is not a remote-CI success claim. Subsequently,
[#213's macOS Bluetooth job](https://github.com/KenAKAFrosty/Prns/actions/runs/34420018083/job/102693180132)
failed at 00:10:49 UTC on the existing USB fixture mismatch: `usb_auto/mod.rs`
lines 708 and 879 pass `UnboundedSender<InterfaceId>` where `ManifoldWakeSender`
is required. This is the defect addressed separately by #199, not code changed
in #213. Other remote checks were still pending at this readback.

## Normal publishing checks

An ordinary atomic push published the two new refs after the existing hook
completed successfully. No hook bypass, force push, source-policy adjustment or
firmware layout/capacity change was used.

The hook ran from the isolated resource-fix checkout, not the integrated app.
Its personal-path check inspected both outgoing tips, while executable checks
tested the current checkout. That distinction matters: the separate canonical
notices check also passed again on the exact notices branch. This is not a claim
that each head independently ran every executable gate.

The successful run includes:

- Hygiene, formatting, documentation links, validation registry and Host SDK
  version/distribution/contract checks.
- All 22 host-compatible workspaces; two real-target workspaces are delegated to
  their embedded gates. The app's earlier 24-workspace result is separate.
- All 41 selected parity gates, including strict linting, the external-allocation
  lane, all 14 configured firmware profiles, browser package smoke, clean
  generated JavaScript, TypeScript contracts, JVM compilation and Swift smoke.
- Dependency/license policy and the canonical unsafe dependency inventory.

The external-allocation core suite passed 1,940 tests with three existing
profiling tests ignored. It is a different configuration from the earlier
1,887-test default Linux suite, not an additional independent population.
Policy warnings remain visible in the log; successful checks do not imply a
warning-free build or physical-device qualification. The hook's selected gates
are not identical to every GitHub job; the macOS failure above makes that limit
concrete. JVM test sources were compiled, not executed. Fuzzing and Kani proofs
were not run by this publication pass.

### Isolated T-Echo measurement

The resource-fix branch's configured T-Echo S140 v7 build uses 622,120 of 622,592
FLASH bytes, leaving **472 bytes**. All fourteen configured profiles fit. This
is a fresh isolated-branch measurement, not an update to the app's historical
616-byte margin or the combined Nordic candidate's 1,224-byte margin. No bare
trunk comparison was rebuilt here, so binary-size equality with trunk is not
claimed. No firmware was flashed or phone binary installed.

## Preserved state and next step

The app's remote branch remains at
`31532838b34bf48158eeef7b0f9b7f2a42706b84`, and #197 remains a collaboration draft.
The five existing heads (#199, #202, #207, #208 and #209) were read back unchanged.
Their refreshed local candidates and proposed rescue refs were not published.
The integrated app's core pin remains `451e669da`.

After these corrections reach trunk, refresh the dependent candidates and rerun
their applicable checks. Reprepare the exact old/new commit and rescue/lease
plan before replacing any shared heads; this publication did not authorize
those rewrites, an app push or the other unpublished follow-up PRs.

Private logs, posted body files and configured firmware reports are retained at
`/Volumes/wavlink/dev/prns-app-acceptance-20260909/ci-publication.SndNlg/`.
The separately repeated notices check is in
`../notices-audit.cYXy0h/notices-publication-check.log`. Build caches stayed on the
external drive. Three temporary artifact-directory symlinks were removed after
the push, returning the contribution worktree to a clean state; the artifacts
themselves remain available.

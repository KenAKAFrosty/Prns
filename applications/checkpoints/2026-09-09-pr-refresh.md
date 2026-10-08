# Five prerequisite PRs refreshed — September 9, 2026

Continuation of the [independent CI publication](2026-09-09-ci-publication.md).
The user approved publishing the reviewed USB/iOS updates and revising their PR
descriptions. Upstream remained at
`1d4d4ba8652875ee6fda835c633398d1bab3419a` before and after publication.

| PR | Published head | Verified new commit | State |
| --- | --- | --- | --- |
| [#199 — Update USB Auto tests for the shared wake signal](https://github.com/KenAKAFrosty/Prns/pull/199) | `codex/fix-usb-auto-wake-tests` | `a0ba517957fc4a027ed625d14ef7bc30208f965a` | Open |
| [#202 — Let iOS apps configure CoreBluetooth state restoration](https://github.com/KenAKAFrosty/Prns/pull/202) | `prns-app-corebluetooth-manager-policy` | `47241e744334a2e4aaa79090bc3c3d5b4c1dccc4` | Open |
| [#207 — Resume Bluetooth sessions restored by iOS](https://github.com/KenAKAFrosty/Prns/pull/207) | `prns-app-corebluetooth-restored-session` | `799c09690b19dbe279c5af2280c4669978cd8d63` | Draft |
| [#208 — Add central-only CoreBluetooth Auto support](https://github.com/KenAKAFrosty/Prns/pull/208) | `prns-app-corebluetooth-role-policy` | `243a153d5ce635aa73625ae3dece057f6c59a394` | Draft |
| [#209 — Clarify CoreBluetooth scan-startup logs](https://github.com/KenAKAFrosty/Prns/pull/209) | `prns-app-corebluetooth-scan-startup` | `0615cb39810de62c61babcc27670f3cb1d97fd79` | Draft |

All still target `trunk`. Titles and draft states were preserved. Concise bodies
retain the app-development motivation, own-change comparisons and qualification
limits. Each body was read back byte-for-byte; all three updated comparison URLs
resolve to the intended parent and child commits.

## Protected update and source correspondence

Five backup branches under `codex/rescue/` were published and verified at the old
remote tips before any replacement. The subsequent five-head push was atomic
and used an explicit old-commit lease for every head. No hook bypass or broad
force option was used. Original local branches/worktrees remain at their old
tips; work from the reviewed `codex/refresh-*` candidates or fetched remote heads
rather than assuming those original local checkouts moved.

All fourteen source patches retain exact range-diff matches. The additional
refreshed #207 commit only updates its canonical unsafe inventory. No new product
source changes were needed during this publication pass.

## Fresh validation and cache ownership

An initial backup-push attempt reused a compiled target from another worktree.
Its test output included #213's continuation regression, absent from the intended
#209 source. Timestamp-based fingerprints had accepted older files, and the
reused firmware tool embedded the previous checkout's root at compile time.
That attempt was stopped before publishing any backup; none of its executable
results is accepted as validation of these candidates.

Both accepted runs used separate, initially empty external targets, with actual
source paths and branch-specific test output verified:

| Publishing run | Executed checkout | Results |
| --- | --- | --- |
| Backup push | #209 at `0615cb398` | 22 host workspaces and all 41 selected parity gates passed |
| Exact-lease replacement push | #199 at `a0ba51795` | 22 host workspaces and all 14 selected parity gates passed |

Both runs completed their entire selected firmware matrix: fourteen configured
profiles each, with no layout or capacity changes. T-Echo S140 v7 measured
622,120 image bytes / 472 free on #209 and 622,112 / 480 free on #199. These are
per-build measurements, not an app firmware result or binary-equivalence claim.
ESP32 images can retain absolute source filenames in loadable data, so checkout
paths can affect their size even when embedded source is unchanged.

Both core external-allocation runs passed 1,939 tests with three existing
profiling ignores, plus the two crypto and two documentation tests. Fresh #209
FFI tests passed 78 tests with one hardware-only ignore in each default/log
configuration. Fresh #199 all-feature/all-target tests passed 280 unit tests
(including 17 USB tests) and one integration test; strict Clippy also passed.
Prior iOS cross-compilation results remain identified in the preparation record.

The accepted hooks cover their selected runtime/browser/dependency checks; #209's
full selection also covered JVM compilation and four Swift tests. JVM test
sources were compiled, not executed. These are not five independent full-matrix
runs, physical-device acceptance, fuzz/proof execution or a complete GitHub CI
result. No phone or board was installed, flashed or otherwise changed.

## Remaining work

Fresh GitHub jobs were queued/running at the post-update readback around 01:34
UTC September 10 (September 9 locally). Separate CI prerequisites #213 and #214
remain open and were not folded into these heads. The iOS heads still need #199
for broad USB-enabled test builds. Future upstream changes require a new ref
readback and protected update plan, not reuse of these old leases.

The app remote remains `31532838b34bf48158eeef7b0f9b7f2a42706b84`; #197 was not
pushed or edited. Native recovery, peripheral write batching and the combined
Nordic diagnostics/footprint candidate remain separate unpublished work.

Private evidence and original/posted PR payloads are under
`/Volumes/wavlink/dev/prns-app-acceptance-20260909/pr-update.ULrel3/`.
The two accepted logs are `rescue-push-fresh.log` and `replacement-push.log`;
`rescue-push.log` is the discarded initial attempt. The execution script and
ledger preserve exact old/new tips and backup names. Temporary artifact links
were removed after the checks; each contribution worktree is clean, and external
logs/build artifacts remain available. Unrelated main-worktree files were kept.

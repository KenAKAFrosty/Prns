# Separate CI corrections — September 9, 2026

This records the pre-publication checkpoint. The later
[publication record](2026-09-09-ci-publication.md) supersedes its unpublished
status and adds the normal publishing-gate and firmware-matrix results.

Continuation of [publication preparation](2026-09-09-publication-preparation.md).
The Linux request failure and generated-notice drift now have separate local
corrections. Neither belongs in the USB fixture or Bluetooth contributions.
No published head, PR, phone binary or board firmware changed.

## Split-resource continuation

Unmodified upstream trunk `1d4d4ba8652875ee6fda835c633398d1bab3419a`
reproduced the split-response TCP failure on trial four after three passes:
`Failed(LinkClosed)`, after 7.47 seconds. A separate failure-only diagnostic run
captured `Rejected(Resource(Timeout))` in the server response path, followed by
its existing deliberate link closure. Adding more logging changed the timing;
passing instrumented runs were not treated as a correction.

A continuation whose predecessor has already completed reserves the live build
lane. The offloaded build initialized its original resource hash to its own
segment hash, but that lane never restored the first segment's hash. The inline
and staged paths already did. A receiver cannot associate that advertisement
with the existing split assembly.

The six-line correction copies the existing chain hash before advertising a
live continuation. It changes no timeout, retry, scheduler, wire format or
ownership boundary. The deterministic regression proves segment one before
reserving segment two, checks the decrypted advertisement and completes both
segments. It failed before the correction and passes afterward. The actual bad
advertisement was not captured in the intermittent TCP failure; the deterministic
wire test establishes the specific defect, and the repeated TCP runs verify the
corrected behavior under the previously failing environment.

| Contribution | Branch / revision |
| --- | --- |
| Isolated core fix on trunk | `codex/fix-live-resource-continuations` at `19106615d7d80a6d021359c5aa02a6bad889edb9` |
| Exact app cherry-pick and new core pin | `451e669da33ec885f3c03cdb2ba26f8ba984caba` |
| Isolated notices refresh on trunk | `codex/fix-third-party-notices` at `e675bc95ae9466cb7d6f17e59f0fe236afe9fc30` |
| App notices integration | `4494a4aac2035c229f3d0b50b1b1aca371b01eb5` |
| App-only snapshot usage notice | `25a04fcd8636206111b77ae4262b23e1798e3725` |

The core cherry-pick has an exact range-diff match and no conflicts. This fixes
general split-resource transfer, relevant to large responses and future app
Resource support. It does not add large-message support to the current small
direct-message implementation or establish that its current flow was affected.

## Validation

The core contribution was tested on Linux arm64, Ubuntu 24.04, Rust 1.98.1,
limited to two CPU cores and 4 GiB RAM. This reproduces the symptom on Linux,
but is not the x86_64 GitHub runner or a complete repository feature matrix.

- New deterministic test: failure before the fix; success afterward.
- Focused resource suite: 222 passed.
- Entire default core library suite: 1,887 passed; three profiling tests ignored.
- All nine integration test targets: 25 passed; one manual interop server ignored.
- Original split-response test: 50 consecutive passes without diagnostic probes.
  The test itself is unchanged; no retry or longer deadline was added.
- Host strict core Clippy, Rust 2021 formatting and whitespace checks passed.
- Core `no_std` compilation for `thumbv7em-none-eabihf` passed, both without
  default features on the isolated fix and with `embassy-host,flash` on the app.
  This is not a new Nordic firmware size measurement or hardware qualification.
- Isolated and integrated canonical unsafe-inventory checks passed without
  changing either snapshot.
- Integrated `native:verify` passed: compatibility, generated outputs/provenance,
  136 native unit tests, four integration tests, platform source checks and
  49 SDK tests. The explicit iOS native-composition cross-check also passed.

No generated API, JavaScript product code or platform admission policy changed.
The previous physical observations remain attached to their recorded binaries,
not this updated source pin. The new pin is local; detached export and public
source retrieval were not rerun. Full remote CI and publishing gates still need
to run against the eventual published heads.

The changed successful-build completion is called by Tokio. Embassy routes
owning resource-build work through `resume_resource_build_unavailable`, not the
changed helper. That call-path distinction, rather than feature absence alone,
justifies the bounded Nordic compilation check here. Normal publishing gates
still select firmware validation for core changes; this does not waive them or
update the historical 616-byte integrated T-Echo margin.

## Generated notices

Four existing dependencies were added to trunk after its last notices refresh:
`personal-hopspot-builder`, `personal-hopspot-memory`,
`personal-hopspot-resources` and `rustc-demangle`. Canonical generation on current
trunk reproduced exactly the earlier CI drift. The isolated fix changes two
usage-list lines under existing MIT texts; no dependencies, license text,
policy, release-graph list or generator behavior changed.

After integrating that refresh, the app branch additionally needed
`prns-host-snapshot` in its registered Host SDK native graph's usage list. That
one-line app-specific adjustment is a separate commit, not part of the trunk PR.

Both canonical fresh-cache checks pass using Rust/Cargo 1.96.0 and
cargo-about 0.9.1. Five existing generator regressions and license-policy parity
pass. These checks cover the 28 registered release graphs, not a complete
shipping-license inventory for the separate applications workspace.

## Evidence and next boundary

Private logs and reproducible commands are under
`/Volumes/wavlink/dev/prns-app-acceptance-20260909/`:

- `linux-ci-repro.Ec8OS7/`: failing baseline, deterministic negative control,
  corrected Linux suites/repeats and app integration checks.
- `notices-audit.cYXy0h/`: canonical before/after checks, precise usage-list
  comparisons, artifact hashes and generator tests.

The disposable Linux test container was removed after validation, releasing its
approximately 10 GiB of internal compiler/build caches. Evidence remains on the
external drive; unrelated containers, images and volumes were not removed.
Reclaiming those container files does not establish a matching reduction in the
host's sparse Docker disk image.

The ignored local publication plan links concise drafts for these two new,
independent PRs. Existing five-head replacement candidates, originals and rescue
proposals are unchanged. Publishing these fixes and replacing shared heads still
require approval of the exact update scope. No push or PR creation/edit occurred.

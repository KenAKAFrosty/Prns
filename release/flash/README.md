# Hopspot flasher release custody

`boards.json` is the authoritative catalog used by the firmware builder, standalone CLI, hosted
website, and validation tools. Published schema-2 manifests contain sparse immutable firmware
parts. The ESP application, partition table, and bootloader are separate files; the `0xD000`
provisioning slot is never part of ordinary firmware.

The S3 per-board artifact gates are pinned to the previous merged-image baselines (7,643,152 bytes
for Heltec V4 and 7,639,296 bytes for T-Beam Supreme) and reject anything above 40% of those
totals. The aggregate ESP gate also includes the XIAO ESP32-C6 baseline (1,309,056 bytes) and
requires at least a 60% reduction across all three ESP boards; XIAO is aggregate-only because its
old image contained little address-gap padding. The embedded SoftAP build excludes the hosted
flasher engine and published firmware.

## Custody layers

The release is intentionally split into three immutable layers:

1. The candidate workflow performs two fresh build/audit passes from the same commit, finalizes
   each candidate independently, and requires identical payload files and deterministic archive
   bytes (including MSVC `/Brepro` for the Windows PE/COFF archive). It adds
   `metadata/reproducibility.json` only after that comparison and uploads one
   unsigned candidate Actions artifact. It does not create or update a GitHub Release.
2. The protected signing workflow downloads that artifact by workflow run ID, checks its
   maintainer-supplied SHA-256, binds it to a successful default-branch candidate run, validates
   `VERSION`, source commit, key ID, manifest parts, hosted copies, CLI archives, audit evidence,
   and checksum coverage, then signs without rebuilding. The signer implementation must be the
   candidate source commit. It creates a deterministic signed archive, preserves the validated run
   ID/attempt as versioned evidence, and attests canonical name/SHA-256 pairs for the archive, five
   CLI archives, and every manifest-referenced firmware payload through GitHub/Sigstore before
   publishing an immutable public prerelease. A second, secret-free job then enters the protected
   `public-release` environment. After its 1,440-minute gate it re-fetches the still-public exact
   bundle and manifest and publishes immutable, attempt-specific review evidence on the GitHub
   prerelease. That persistent asset binds the signing run, exact rerun attempt, protected job,
   source revision, candidate hashes, and review interval. The prerelease is therefore visible
   before the review clock can begin, and promotion does not start a second wait.
3. After physical qualification, the protected evidence workflow validates schema-2 acceptance,
   signs it, generates a release record binding every custody layer, signs that record, and adds the
   deterministic qualification-evidence archive plus four signed evidence documents to the
   prerelease. It also revalidates one exact successful public-review run attempt and binds that
   durable evidence asset into the signed release record. Protected promotion trusts only that
   signed record.

The reproducibility gate compares unsigned release payloads and deterministic archives. Minisign
signatures, Sigstore envelopes, signed acceptance, and the signed release record are generated
later and are recorded as separate custody envelopes. Minisign trusted comments bind the document
SHA-256 rather than wall-clock time, allowing an interrupted protected upload to reproduce exact
bytes; Sigstore retains its independent timestamped provenance. Packaging an already-signed
directory remains deterministic: the same files produce the same signed-candidate archive bytes.

Production candidate jobs force Rust 1.96.0, Node 24.18.0, Dioxus CLI 0.7.5,
`cargo-binstall` 1.21.0, `espup` 0.17.1, ESP Rust 1.95.0, the SHA-256-pinned Espressif
crosstool-NG `15.2.0_20250920` archive (GCC 15.2.0), and the Rust 1.96 `llvm-tools-preview`
`llvm-objcopy`. Windows CLI links also force MSVC `/Brepro`, matching Rust's own reproducible-build
test requirement for suppressing PE/COFF linker timestamps. The website runtime graph keeps
`esptool-js` 0.6.0 and `spark-md5` 3.0.2 exact; Playwright 1.61.1 and axe 4.12.1 are exact-pinned,
audited test-only tools
and are rejected from production sources and bundles. `release/flash/action-pins.json` records the
reviewed full Git commit for every third-party workflow action.

## Operator gates

Before signing any candidate:

- complete the repository history privacy/secret audit and make the repository public;
- replace the public-key custody marker and retain the encrypted offline recovery copy;
- protect the default branch and create the `release-signing` environment described in
  `release/keys/README.md`;
- create `public-release` with manual approval and a 1,440-minute wait timer;
- create `release-rollback` with manual release-owner approval and no signing secrets or wait
  timer; rollback jobs only receive the repository public key and read-only release inputs;
- confirm Actions attestations are available for the repository and `gh attestation verify` works;
- assign trusted testers for macOS arm64/x86_64, Linux arm64/x86_64, and Windows x86_64;
- commit and validate `release/acceptance/rosters/VERSION.json` with all five real assignments;
- review the exact default-branch workflow revisions. Do not dispatch a signing workflow from a
  feature branch.
- require the stable **Release critical** CI check in default-branch protection. It includes the
  Rust 1.90 MSRV, Rust/CLI contracts, fake serial tests, Chromium/axe browser tests, production
  fixture exclusion, release-script tests, and dependency/notices gates.

These are operator-controlled GitHub settings. Repository workflows fail closed when the public
key, environment secret, public visibility, attestation support, or required release assets are
absent; they do not create or weaken those settings.

## Build and sign the exact candidate

1. Dispatch **flasher release candidate** on the default branch with the intended channel, pinned
   public key ID, and one explicit website-history mode. `retain` is the default and requires the
   exact current stable version plus the lowercase SHA-256 of its signed release record. The
   workflow verifies that release, its complete asset set, attestations, and the live signed stable
   descriptor before carrying every immutable `/releases/VERSION` directory forward. Both
   independent candidate builds consume the same hash-verified history archive. `bootstrap` is
   permitted only when GitHub has no schema-v2 signed candidate/release-record asset and the live
   stable URL does not contain a canonical schema-1 descriptor; the current coming-soon HTML
   fallback counts as absent, while a network error fails closed.
2. Download `prns-flasher-candidate-vVERSION-unsigned.tar.gz`, calculate its lowercase SHA-256, and
   review the candidate/audit output, `metadata/sparse-sizes.json`, and
   `metadata/reproducibility.json`. Record the workflow run ID and exact hash independently. The
   signed candidate also carries the exact committed tester roster, qualification guide, offline
   website server, acceptance generator/validator, and platform-neutral candidate-file verifier.
   Candidate construction fails if the versioned roster is absent, uncommitted, or incomplete. The
   candidate workflow is expected to remain blocked while the committed Minisign public key still
   carries the fail-closed custody marker.
3. Dispatch **sign exact flasher candidate** with those two values. The workflow rejects candidates
   from forks, feature branches, failed runs, other workflows, mismatched commits, mutable hashes,
   or the unconfigured key marker. It never invokes a firmware, CLI, or website build.
4. Confirm the resulting `vVERSION` GitHub Release is a non-draft prerelease targeting the manifest
   source commit. The same signing run now waits in its secret-free **Complete protected 24-hour
   public review** job. Authorize the `public-release` environment only after the prerelease and its
   direct qualification/audit assets are visible. Record its publication time, signed candidate
   SHA-256, manifest SHA-256, candidate workflow-run evidence, attestation URL, and signing workflow
   run. That review job independently refuses to emit evidence until the prerelease has actually
   been public for 24 hours. Successful attempts publish distinct
   `public-review-vVERSION-run-RUN_ID-attempt-ATTEMPT.json` assets and never replace earlier
   evidence. Finalization examines those persistent identities in attempt order and accepts only one
   whose exact attempt-specific workflow and protected-job APIs report success. Promotion and
   historical verification then revalidate only that signed selection. A later rerun therefore
   cannot silently shadow the reviewed attempt.

The signing workflow may replace only an unpublished matching draft created by an interrupted run;
it validates the draft identity and all staged bytes before making it public. It refuses to replace
an existing public release or unrelated tag. If anything affecting release bytes or custody is
wrong, correct it, advance the candidate/version as required, and start a new candidate. Never
overwrite a candidate that testers may already have used.

A bootstrap candidate establishes deterministic first-release inputs, but it is not promotable.
Promotion requires its candidate history to name an exact prior signed stable baseline and requires
a successful rollback dry-run against that baseline. Before the first public promotion, create,
publish, and deploy a signed baseline through an approved custody procedure, then rebuild the
release candidate in `retain` mode. If no such baseline is approved, first promotion is a go/no-go
blocker; changing that rule requires an explicit product-policy decision, not a workflow
workaround.

## Qualify and finalize evidence

Testers extract the signed candidate. CLI qualification imports/uses only its verified cache
contents; web qualification serves `CANDIDATE/website` from localhost and opens `/flash`. Hardware
results follow `release/acceptance/README.md`. No unsigned or locally rebuilt artifact counts.

Commit the completed record at `release/acceptance/records/VERSION.json` through normal review.
Every tester must match the exact OS/architecture assignment in the signed candidate roster, and
every `completed_at` must be a full UTC timestamp no earlier than the exact prerelease
`publishedAt`. Redact the reviewed evidence objects, store each under its SHA-256 name, package
them with the candidate's deterministic qualification-evidence packager, and upload the resulting
`qualification-evidence-vVERSION.tar.gz` once to the prerelease. Then dispatch **finalize flasher
release evidence** with the exact version, full default-branch acceptance commit, and independently
recorded evidence-archive SHA-256. The workflow:

- proves that evidence commit is on the default branch;
- downloads rather than rebuilds the signed prerelease and exact qualification-evidence archive;
- verifies candidate Minisign signatures and GitHub attestations;
- revalidates one durable public-review evidence asset against its exact workflow run attempt and
  protected job;
- extracts the evidence archive safely and recomputes every referenced object's SHA-256;
- validates the transport-aware 24-row matrix, browser fallbacks, and all five installer/doctor
  smokes;
- signs `acceptance-vVERSION.json`;
- creates and signs `release-record-vVERSION.json`.

The release record byte-compares the supplied candidate directory with a safe extraction of the
signed archive, then binds the release version/channel/source commit/key ID, signed candidate archive,
candidate-run evidence hash and parsed repository/workflow/run/attempt/commit identity, manifest and
signature, channel descriptor and signature, checksum document and signature, build metadata,
dependency-audit evidence, exact tester-roster hash, acceptance record and
signature/evidence commit, exact signed-candidate hash, prerelease publication instant,
qualification-evidence archive name/size/SHA-256, Sigstore bundle hash,
attestation ID/URL/workflow identity, every attested subject hash, and a path/size/SHA-256 identity
for every sparse firmware payload in the signed manifest. It also binds the exact immutable
public-review evidence asset and its workflow run, rerun attempt, protected job, source revision,
and completion instant.

## Prove rollback readiness

Every signed stable release keeps its complete candidate bundle and complete website as immutable
GitHub Release assets. A retained candidate additionally carries all prior immutable hosted release
directories, so a new site never drops old manifest or firmware URLs.

Before promotion, dispatch **verify or deploy an exact flasher rollback** in `dry-run` mode with:

- the prior signed stable baseline version and exact release-record SHA-256;
- the release being promoted as `expected_live_version` and its signed manifest SHA-256 as
  `expected_live_manifest_sha256`;
- empty `dry_run_id` and `dry_run_attempt` values.

The dry-run downloads and independently verifies every baseline custody asset and attestation,
requires the signed live descriptor to identify that exact baseline, stages the complete prior
website, records its exact tree identity, and must complete successfully within 15 minutes. It
deliberately records that the compare-and-swap away from the future release is deferred: the new
release is not live yet. Preserve the successful workflow run ID and exact attempt; promotion
redownloads the baseline, reconstructs the same staged tree, and validates the attempt-specific
artifact, run, protected job, and dry-run record rather than trusting operator-supplied evidence.

## Promote

After the signing run's protected public-review job has completed successfully, the prerelease has
been public for at least 24 hours, and every stop-ship report is resolved,
calculate the signed release record's SHA-256 and dispatch **promote signed flasher release** with
the version, that hash, the rollback baseline version, its exact release-record SHA-256, and the
successful rollback dry-run run ID and exact attempt. The protected workflow independently
re-verifies Minisign, all
candidate hashes, physical acceptance, release-record equality, GitHub attestations, stable
channel, source commit, public release state, public-review interval, complete rollback baseline,
the immutable review artifact plus its exact successful signing workflow/job revision,
retained-history head, and the successful 15-minute dry-run before deploying the exact website
bundle. Before deployment it downloads the complete prerelease asset set and verifies every
expected byte; the stable/latest metadata edit compares the full name/size/digest inventory before
and after. The serialized Pages job makes the signed stable descriptor live as part of that exact
bundle, re-fetches and cryptographically verifies the live descriptor and manifest, and compares
the live website shell and flasher engine to the candidate before marking the GitHub Release
stable/latest. The verification job preserves distinct, attempt-bound candidate and baseline Pages
artifacts plus the baseline's complete website identity. If deployment, live verification,
release mutation, post-promotion smoke, or the final public-site marker fails, an `always()` recovery
job acquires the same Pages custody group, compare-and-swaps only the failed candidate (or an
idempotently restored baseline), redeploys the exact baseline artifact, verifies every live file,
rechecks both releases against their previously verified asset inventories, restores the baseline
as latest, and only then demotes the failed candidate to a prerelease. A concurrently promoted third
identity blocks that recovery instead of being overwritten. A rerun consumes the original
verification attempt's artifact names, so retrying failed jobs cannot silently select different
bytes or collide with an earlier artifact.

Promotion never rebuilds or replaces release assets. A missing/tampered acceptance document,
release record, attestation, signature, expected hash, or physical result blocks deployment.
After deployment, the workflow fetches and verifies the live signed channel and manifest, compares
the deployed website shell and flasher bundle with the signed candidate, downloads and hashes every
live firmware part, checks the complete release-asset set, and exercises the immutable Linux shell
installer.

## Roll back

To restore a prior release, first run **verify or deploy an exact flasher rollback** in `dry-run`
mode using the immutable target version/release-record hash and the exact currently live
version/manifest hash. After that run succeeds within 15 minutes, dispatch the same workflow in
`deploy` mode with identical identities, its `dry_run_id`, and its `dry_run_attempt`. The protected workflow re-verifies
the complete target release and dry-run evidence, then checks the signed live stable descriptor as
a compare-and-swap immediately before deployment. An interrupted rerun may accept the exact target
descriptor only as an idempotent resume; any third identity blocks it.

Rollback deploys the target's entire stored website artifact without rebuilding, re-fetches and
byte-checks every path in the staged website inventory, verifies the live signed descriptor and
manifest, and only then marks the prior GitHub Release latest. It compares the target
release's asset inventory before and after that metadata change, so assets are never rewritten or
mixed across versions. The rollback workflow never receives or uses the Minisign signing secret.
The coming-soon site shares the same serialized Pages custody group and permanently refuses to
deploy when the live channel is a valid signed stable descriptor, so a queued prelaunch run cannot
overwrite a promoted or rolled-back site.

## Public verification

The website and CLI verify a signed stable/preview channel descriptor, the immutable manifest, and
every artifact size/SHA-256 before opening a device. Checked installers embed native archive hashes,
install without administrator access by default, and point only at immutable release assets. Apple
notarization and Authenticode are not claimed for this release.

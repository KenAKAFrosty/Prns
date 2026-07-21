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
   publishing an immutable public prerelease.
3. After physical qualification, the protected evidence workflow validates schema-2 acceptance,
   signs it, generates a release record binding every custody layer, signs that record, and adds the
   four evidence documents to the prerelease. Protected promotion trusts only that signed record.

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
- confirm Actions attestations are available for the repository and `gh attestation verify` works;
- assign trusted testers for macOS arm64/x86_64, Linux arm64/x86_64, and Windows x86_64;
- review the exact default-branch workflow revisions. Do not dispatch a signing workflow from a
  feature branch.
- require the stable **Release critical** CI check in default-branch protection. It includes the
  Rust 1.90 MSRV, Rust/CLI contracts, fake serial tests, Chromium/axe browser tests, production
  fixture exclusion, release-script tests, and dependency/notices gates.

These are operator-controlled GitHub settings. Repository workflows fail closed when the public
key, environment secret, public visibility, attestation support, or required release assets are
absent; they do not create or weaken those settings.

## Build and sign the exact candidate

1. Dispatch **flasher release candidate** on the default branch with the intended channel and the
   pinned public key ID.
2. Download `prns-flasher-candidate-vVERSION-unsigned.tar.gz`, calculate its lowercase SHA-256, and
   review the candidate/audit output, `metadata/sparse-sizes.json`, and
   `metadata/reproducibility.json`. Record the workflow run ID and exact hash independently. The
   candidate workflow is expected to remain blocked while the committed Minisign public key still
   carries the fail-closed custody marker.
3. Dispatch **sign exact flasher candidate** with those two values. The workflow rejects candidates
   from forks, feature branches, failed runs, other workflows, mismatched commits, mutable hashes,
   or the unconfigured key marker. It never invokes a firmware, CLI, or website build.
4. Confirm the resulting `vVERSION` GitHub Release is a non-draft prerelease targeting the manifest
   source commit. Record its publication time, signed candidate SHA-256, candidate workflow-run
   evidence, attestation URL, and signing workflow run.

The signing workflow may replace only an unpublished matching draft created by an interrupted run;
it validates the draft identity and all staged bytes before making it public. It refuses to replace
an existing public release or unrelated tag. If anything affecting release bytes or custody is
wrong, correct it, advance the candidate/version as required, and start a new candidate. Never
overwrite a candidate that testers may already have used.

## Qualify and finalize evidence

Testers extract the signed candidate. CLI qualification imports/uses only its verified cache
contents; web qualification serves `CANDIDATE/website` from localhost and opens `/flash`. Hardware
results follow `release/acceptance/README.md`. No unsigned or locally rebuilt artifact counts.

Commit the completed record at `release/acceptance/records/VERSION.json` through normal review.
Evidence references must already be redacted and accessible to release reviewers. Then dispatch
**finalize flasher release evidence** with the exact version and full default-branch commit that
contains the record. The workflow:

- proves that evidence commit is on the default branch;
- downloads rather than rebuilds the signed prerelease;
- verifies candidate Minisign signatures and GitHub attestations;
- validates the transport-aware 24-row matrix, browser fallbacks, and all five installer/doctor
  smokes;
- signs `acceptance-vVERSION.json`;
- creates and signs `release-record-vVERSION.json`.

The release record binds the release version/channel/source commit/key ID, signed candidate archive,
candidate-run evidence hash and parsed repository/workflow/run/attempt/commit identity, manifest and
signature, channel descriptor and signature, checksum document and signature, build metadata,
dependency-audit evidence, acceptance record and signature/evidence commit, Sigstore bundle hash,
attestation ID/URL/workflow identity, every attested subject hash, and a path/size/SHA-256 identity
for every sparse firmware payload in the signed manifest.

## Promote

After the prerelease has been public for at least 24 hours and every stop-ship report is resolved,
calculate the signed release record's SHA-256 and dispatch **promote signed flasher release** with
the version and that hash. The protected workflow independently re-verifies Minisign, all candidate
hashes, physical acceptance, release-record equality, GitHub attestations, stable channel, source
commit, public release state, and public-review interval before deploying the exact website bundle
and marking the prerelease stable/latest. A rerun may resume the exact already-promoted release
after a deploy or smoke interruption; it cannot substitute different assets.

Promotion never rebuilds or replaces release assets. A missing/tampered acceptance document,
release record, attestation, signature, expected hash, or physical result blocks deployment.
After deployment, the workflow fetches and verifies the live signed channel and manifest, compares
the deployed website shell and flasher bundle with the signed candidate, downloads and hashes every
live firmware part, checks the complete release-asset set, and exercises the immutable Linux shell
installer. The protected immutable rollback workflow and its recorded dry-run remain a Wave 4
launch gate. Unless repository history proves there is no prior signed schema-v2 release to
restore, public launch remains blocked until that gate exists and passes; this release lane does
not claim rollback completeness yet.

## Public verification

The website and CLI verify a signed stable/preview channel descriptor, the immutable manifest, and
every artifact size/SHA-256 before opening a device. Checked installers embed native archive hashes,
install without administrator access by default, and point only at immutable release assets. Apple
notarization and Authenticode are not claimed for this release.

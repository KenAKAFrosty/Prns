# Hopspot flasher release custody

`boards.json` is the authoritative catalog used by the firmware builder, standalone CLI, hosted
website, and validation tools. Published schema-2 manifests contain sparse immutable firmware
parts. The ESP application, partition table, and bootloader are separate files; the `0xD000`
provisioning slot is never part of ordinary firmware.

The S3 artifact gate is pinned to the previous merged-image baselines (7,643,152 bytes for Heltec
V4 and 7,639,296 bytes for T-Beam Supreme) and rejects anything above 40% of those totals. The
embedded SoftAP build excludes the hosted flasher engine and published firmware.

## Custody layers

The release is intentionally split into three immutable layers:

1. The candidate workflow builds and audits once, finalizes `SHA256SUMS.txt`, and uploads one
   unsigned candidate Actions artifact. It does not create or update a GitHub Release.
2. The protected signing workflow downloads that artifact by workflow run ID, checks its
   maintainer-supplied SHA-256, binds it to a successful default-branch candidate run, validates
   `VERSION`, source commit, key ID, manifest parts, hosted copies, CLI archives, audit evidence,
   and checksum coverage, then signs without rebuilding. It creates a deterministic signed archive,
   preserves the validated run ID/attempt as versioned evidence, attests the archive and five CLI
   binaries through GitHub/Sigstore, and publishes an immutable public prerelease.
3. After physical qualification, the protected evidence workflow validates schema-2 acceptance,
   signs it, generates a release record binding every custody layer, signs that record, and adds the
   four evidence documents to the prerelease. Protected promotion trusts only that signed record.

Signatures contain Minisign signing metadata and therefore are not expected to reproduce across
independent signing operations. Packaging the already-signed directory is deterministic: the same
files produce the same signed-candidate archive bytes.

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

These are operator-controlled GitHub settings. Repository workflows fail closed when the public
key, environment secret, public visibility, attestation support, or required release assets are
absent; they do not create or weaken those settings.

## Build and sign the exact candidate

1. Dispatch **flasher release candidate** on the default branch with the intended channel and the
   pinned public key ID.
2. Download `prns-flasher-candidate-vVERSION-unsigned.tar.gz`, calculate its lowercase SHA-256, and
   review the candidate/audit output. Record the workflow run ID and exact hash independently.
3. Dispatch **sign exact flasher candidate** with those two values. The workflow rejects candidates
   from forks, feature branches, failed runs, other workflows, mismatched commits, mutable hashes,
   or the unconfigured key marker. It never invokes a firmware, CLI, or website build.
4. Confirm the resulting `vVERSION` GitHub Release is a non-draft prerelease targeting the manifest
   source commit. Record its publication time, signed candidate SHA-256, candidate workflow-run
   evidence, attestation URL, and signing workflow run.

The signing workflow refuses to replace an existing tag or release. If anything affecting release
bytes or custody is wrong, correct it, advance the candidate/version as required, and start a new
candidate. Never overwrite a candidate that testers may already have used.

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
attestation ID/URL/workflow identity, and every attested subject hash.

## Promote

After the prerelease has been public for at least 24 hours and every stop-ship report is resolved,
calculate the signed release record's SHA-256 and dispatch **promote signed flasher release** with
the version and that hash. The protected workflow independently re-verifies Minisign, all candidate
hashes, physical acceptance, release-record equality, GitHub attestations, stable channel, source
commit, prerelease state, and public-review interval before deploying the exact website bundle and
marking the prerelease stable/latest.

Promotion never rebuilds or replaces release assets. A missing/tampered acceptance document,
release record, attestation, signature, expected hash, or physical result blocks deployment.

## Public verification

The website and CLI verify a signed stable/preview channel descriptor, the immutable manifest, and
every artifact size/SHA-256 before opening a device. Checked installers embed native archive hashes,
install without administrator access by default, and point only at immutable release assets. Apple
notarization and Authenticode are not claimed for this release.

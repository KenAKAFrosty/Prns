# Hopspot flasher release custody

`boards.json` is the authoritative catalog used by the firmware builder, standalone CLI, hosted
website, and validation tools. Published release manifests use schema 2 and contain sparse,
immutable firmware parts. The ESP application, partition table, and bootloader are separate files;
the `0xD000` provisioning slot is never part of ordinary firmware.

The S3 artifact gate is pinned to the previous merged-image baselines (7,643,152 bytes for Heltec
V4 and 7,639,296 bytes for T-Beam Supreme) and rejects anything above 40% of those totals. The
embedded SoftAP build is a compact device-setup page; source archives, browser playgrounds,
published firmware, and hosted flasher JavaScript stay in the hosted release only.

## Candidate flow

1. Run the **flasher release candidate** workflow with a channel and the public key ID. It builds
   firmware once, builds the hosted and embedded site variants, packages all five native CLIs,
   audits every Rust/npm graph, and creates an unsigned draft candidate.
2. Download and inspect `prns-flasher-candidate-vVERSION-unsigned.tar.gz` on the offline signing
   workstation. Run:

   ```text
   scripts/sign-flasher-candidate.sh CANDIDATE_DIR OFFLINE_MINISIGN_SECRET_KEY
   ```

   The script signs the exact manifest, channel descriptor, and `SHA256SUMS.txt`. The private key
   remains offline and must never enter this repository, CI, or the candidate.
3. Run all hardware scenarios against that exact signed directory. Copy
   `release/acceptance/template.json` to `acceptance.json`, fill the evidence described in
   `release/acceptance/README.md`, and validate it with
   `scripts/verify-flasher-candidate.sh CANDIDATE_DIR`.
4. Archive the signed directory as `prns-flasher-candidate-vVERSION-signed.tar.gz` and upload it to
   the existing draft GitHub Release.
5. Run the protected **promote signed flasher release** workflow. It verifies signatures, every
   checksum and manifest part, hosted-document parity, the complete board/OS/browser matrix, and
   every published CLI architecture before deploying the stable website and publishing the draft.

The pre-launch coming-soon website is a manual workflow. It cannot overwrite the full site after a
promotion. Rollback is performed by redeploying a previous complete signed candidate; individual
manifests, firmware files, and website assets are never mixed between releases.

## Public verification

The website and CLI first verify a signed stable/preview channel descriptor. That descriptor points
to `https://reticulum.rs/releases/VERSION/flash-manifest.json` and pins its SHA-256. They then verify
the manifest's Minisign signature and every artifact's exact size and SHA-256 before opening a
device. Explicit CLI versions use the same immutable release path directly.

The checked installers embed each CLI archive's exact SHA-256, install without administrator access
by default, and point only at immutable GitHub Release assets. The release makes no Apple
notarization or Authenticode claim.

## Pre-publication qualification

Extract the signed candidate and use the CLI binary from the candidate's native archive. The
release-only qualification input uses exactly the signed channel, manifest, and artifacts without
publishing them first:

```text
hopspot-flash flash BOARD --channel preview --candidate CANDIDATE_DIR --yes
```

For web qualification, serve `CANDIDATE_DIR/website` from localhost and open its `/flash` route.
The candidate build is compiled for its signed channel; on localhost only, immutable
`reticulum.rs` release URLs are resolved to the identical signed files inside the candidate. All
Minisign, manifest-hash, artifact-size, SHA-256, and device-side verification remains active. The
production host never uses this local resolution path.

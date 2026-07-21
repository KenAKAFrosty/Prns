# Flasher release signing key custody

`minisign.pub` is the only key material that belongs in the repository. Replace the explicit
`PRNS_RELEASE_KEY_NOT_CONFIGURED` marker with the public half of the production Minisign key before
building a candidate. Its standard first-line comment (`untrusted comment: minisign public key
KEYID`) is part of the release contract; the manifest's 16-digit hexadecimal key ID must match it.

Generate the production key on an offline maintainer-controlled system. Retain an encrypted
recovery copy on physically separate storage. GitHub Actions receives a separate, unencrypted
signing copy only as the `PRNS_MINISIGN_SECRET_KEY_B64` secret in the protected
`release-signing` environment. The secret must never be configured as a repository-wide secret,
written to an artifact, printed, cached, or committed.

The CI copy is intentionally unencrypted so the protected job can run without storing a second
password beside it. Environment approval, a public reviewed workflow, least-privilege permissions,
and the ephemeral hosted runner are its custody boundary. The signing workflow decodes it into a
mode-0600 temporary file, removes that file on terminal shell paths, and never copies it into the
candidate.

Before adding the environment secret:

1. Make the repository public and finish the repository-history privacy/secret review.
2. Protect the default branch and review the exact `flasher-sign.yml` and
   `flasher-finalize-evidence.yml` revisions on that branch.
3. Create `release-signing` with explicit maintainer approval and restrict it to the protected
   default branch.
4. Base64-encode the unencrypted CI key without line wrapping and provide it through the GitHub UI
   or `gh secret set PRNS_MINISIGN_SECRET_KEY_B64 --env release-signing < ENCODED_KEY_FILE`.
5. Keep the offline recovery copy out of GitHub, developer worktrees, shell history, and cloud-synced
   workspace directories.

Minisign is the client trust root. GitHub/Sigstore provenance is generated for the signed bundle and
CLI archives as independent supplemental evidence; it does not replace the pinned Minisign key.

# Offline release signing key

`minisign.pub` is the only key material that belongs in this repository. Replace the explicit
`PRNS_RELEASE_KEY_NOT_CONFIGURED` marker with the public half of the maintainer-controlled offline
Minisign key before producing a release candidate.
The standard first-line comment (`untrusted comment: minisign public key KEYID`) is part of the
checked custody contract; the manifest's 16-digit hexadecimal key ID must match it.

Never create, copy, or store the secret key in this repository or in GitHub Actions. Candidate
artifacts are downloaded for offline signing; protected promotion verifies the returned signatures
using this pinned public key.

# Host SDK release administration

Staging does not require registry credentials. Public promotion requires the
following owner-controlled setup.

## GitHub

- Create a protected `host-sdk-release` environment with required approval and
  allow deployments only from `main`.
- Protect stable release tags from movement or deletion.
- Enable immutable GitHub Releases and set the repository variable
  `HOST_SDK_IMMUTABLE_RELEASES=enabled` only after that policy is active.
- Create a release-only SSH signing key without a passphrase, register its
  public half as a signing key on the release owner account, and keep the
  private half as the protected environment secret `HOST_SDK_SSH_SIGNING_KEY`.
  Set `HOST_SDK_SIGNING_NAME` and `HOST_SDK_SIGNING_EMAIL` as protected
  environment variables for the same owner identity.
- Keep fallback credentials environment-scoped. The publishing jobs request
  GitHub OIDC identities only after the package proofs pass.
- Run `host-sdk-stage` with the exact approved `main` SHA. After its complete
  stage passes, dispatch `host-sdk-promote` with that SHA and the successful
  stage run ID. Promotion re-verifies the stage, signs every asset and all four
  ecosystem tags, and refuses to replace an existing release.
- After the separately approved registry jobs and Julia General registration
  complete, dispatch `host-sdk-public-qualification` for the same SHA. It
  verifies the public asset and tag signatures, installs each registry package
  in clean consumers, repeats the JavaScript and .NET persistent two-node
  journey, and exercises the released C, C++, Go, Swift, Julia, Python, Maven,
  and Rust surfaces.

## npm

- Create an npm account with two-factor authentication and claim
  `personal-rns` plus all eight `personal-rns-<platform>` packages listed in
  `packages.json`.
- Configure a trusted publisher on each package with GitHub owner
  `KenAKAFrosty`, repository `Prns`, workflow `napi.yml`, and environment
  `host-sdk-release`.
- The workflow uses Node 24 and GitHub OIDC. It does not require `NPM_TOKEN`
  after trusted publishing is active for all nine packages. A granular
  `NPM_TOKEN` restricted to those packages may be kept temporarily during
  rollout, then revoked after the first OIDC publication succeeds.

## PyPI

- Create a PyPI account with two-factor authentication.
- Create or claim `personal-rns`. A pending trusted publisher may create the
  project on its first release.
- Configure GitHub owner `KenAKAFrosty`, repository `Prns`, workflow
  `host-sdks.yml`, and environment `host-sdk-release` as the trusted publisher.
- Do not create `PYPI_API_TOKEN`; publication exchanges GitHub OIDC for a
  short-lived PyPI credential.

## NuGet

- Create a NuGet.org account with two-factor authentication and create or claim
  `PersonalRns`.
- When Trusted Publishing is available, configure GitHub owner
  `KenAKAFrosty`, repository `Prns`, workflow `host-sdks.yml`, and environment
  `host-sdk-release`. Add the NuGet.org username as the GitHub environment
  variable `NUGET_USER`.
- If Trusted Publishing is not yet offered on the account, add a
  package-scoped push-only API key as the environment secret `NUGET_API_KEY`
  and leave `NUGET_USER` unset. Remove the secret after migrating to OIDC.

## Maven Central

- Create a Central Portal publisher account.
- Verify ownership of the `io.reticulum` namespace. If that namespace cannot be
  verified, change the group ID before the first public publication; a released
  coordinate cannot be renamed in place.
- Create a release-only OpenPGP signing key whose public identity is published.
- Generate a Central Portal user token. Store the token, armored private signing
  key, and passphrase as `MAVEN_CENTRAL_BEARER_TOKEN`, `MAVEN_SIGNING_KEY`, and
  `MAVEN_SIGNING_PASSWORD` only in the protected `host-sdk-release` environment.

## crates.io

- Create a crates.io account through GitHub and verify its email address.
- Confirm ownership of `personal-rns`, then reserve or publish the eight
  required `prns-*` crates in the dependency order recorded in `packages.json`.
- Create a crates.io token scoped to publishing the nine named crates and keep
  it as `CARGO_REGISTRY_TOKEN` only in the protected release environment.

## Julia, Go, Swift, C, and C++

- Julia General registration uses the repository owner’s GitHub identity and a
  Registrator request for the package subdirectory.
- Go and Swift require no registry accounts. Signed Git tags and immutable
  GitHub release assets are their distribution authority.
- C and C++ use the signed `host-sdk-v<version>` release. Each target archive
  carries the ABI header, dynamic library, static library, checksums, package
  metadata, and licenses. Verify `SHA256SUMS` and the adjacent SSH signature
  before installation.

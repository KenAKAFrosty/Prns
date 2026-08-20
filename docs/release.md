# Release and version policy

Prns is pre-1.0 software. The current suite version lives in the repository
root [`VERSION`](../VERSION) file and is stamped into the docs site together
with the exact source commit.

## Build provenance

Release builds should stamp both a version and a commit:

- `PRNS_BUILD_VERSION`: overrides the value read from `VERSION`.
- `PRNS_BUILD_COMMIT`: overrides the full source commit detected by `git`.
- `PRNS_BUILD_COMMIT_SHORT`: overrides the displayed short commit. When it is
  not set, the docs build derives it from the full commit.

The docs footer displays the public version and the short source snapshot. The
full commit is kept in the footer title text. The release candidate process
packages that exact commit directly into the hosted website as `source.zip`
plus `source.zip.sha256`; ordinary Dioxus builds never write or inherit those
release artifacts.

The ZIP is the one authoritative full-repository source snapshot. It includes
the website implementation under `docs/website/` and the NomadNet page source
under `personal-hopspot/core/src/node_pages.rs` and
`personal-hopspot/core/src/node_pages/`. Candidate validation regenerates the
ZIP from the stamped commit byte-for-byte, checks its SHA-256 sidecar, and
requires both source areas before signing. To package the current checkout
manually:

```sh
./tools/prns release source package -- --output target/source.zip
```

After the Rust toolchain is installed, the equivalent Cargo convenience command
is:

```sh
cargo tools release source package -- --output target/source.zip
```

Official candidate creation performs this packaging before any website, browser
playground, or firmware release build. It also writes
`metadata/source.json`, containing the canonical version, full commit, byte
length, SHA-256, and NomadNet routes. The hosted website, browser, desktop,
Android, and iOS release builds receive that identity through
`PRNS_SOURCE_ARCHIVE`, `PRNS_SOURCE_VERSION`, `PRNS_SOURCE_COMMIT`,
`PRNS_SOURCE_SIZE`, and `PRNS_SOURCE_SHA256`; enabling their `source-archive`
Cargo feature without all five matching values fails the build.

Embedded Hopspot firmware deliberately remains compact and does not embed or
serve the multi-megabyte source archive. Every board omits embedded source
metadata from its flash target and records `status: "absent"` in
`metadata/source-capabilities.json`. The exact archive remains available from
the hosted release surfaces. This keeps embedded resource memory bounded and
preserves flash capacity for dual-slot OTA work.

## Build the portable daemon

From the repository root, build the canonical local `prnsd` artifact with:

```sh
cargo prnsd build
```

This performs a locked release build with the optional OTLP support compiled
in and prints the absolute path to the executable. The normal paths are
`prnsd/target/release/prnsd` on macOS and Linux and
`prnsd\target\release\prnsd.exe` on Windows. OTLP export remains inactive
unless an endpoint is configured.

The printed executable is self-managing and can be copied to another location
on the same platform. It does not need Cargo or a repository checkout at run
time. Build options such as `--target` or `--profile` can be supplied after
`cargo prnsd build`; the no-option command is the canonical local artifact.

| Command | Behavior |
| --- | --- |
| `prnsd` or `prnsd start` | Start if needed, show the visual header, and attach to the log |
| `prnsd --detach` | Start if needed, wait for readiness, and return to the shell |
| `prnsd restart [OPTIONS]` | Gracefully replace the managed daemon |
| `prnsd logs` | Show recent output and follow the running daemon; return status 3 when stopped |
| `prnsd stop` | Show recent output, request graceful shutdown, and follow the final logs |
| `prnsd run [OPTIONS]` | Run in the foreground; `--service --config DIR` registers with a native service manager context |
| `prnsd i2p doctor` | Check I2P router and SAM 3.1 readiness without starting the managed daemon |
| `prnsd i2p setup` | Print guided platform installation, SAM enablement, and a validated interface stanza |
| `prnsd interfaces [COMMAND]` | Guided typed interface editing, grouped validation and repair, and explicit live apply |

`prnsd status` is the prefixless RNS network-status utility, not a managed-process status command.
It and the other RNS 1.4.2-compatible one-shot utilities are documented in
[`docs/prnsd-utilities.md`](prnsd-utilities.md).

`prnsd` and `cargo prnsd` share one per-user managed session. Repeated starts
reattach without starting another process, and Ctrl-C detaches without stopping
the daemon. Set `PRNSD_STATE_DIR` to create an isolated session for testing or
advanced multi-instance use.

Release builds include the default `tray` feature. Once the daemon is ready,
the Prns mark appears in the macOS, Windows, or Linux system tray. Its menu
shows live interface health, opens an attached Prns terminal, runs network
status or the guided interface editor, reveals the effective configuration
folder, and stops the daemon through the normal graceful shutdown path,
including the final persistence flush. Direct `prnsd run` sessions are labeled
as foreground sessions and do not offer managed-log attachment. A missing
desktop session or Linux StatusNotifier watcher only disables the tray and
records `tray_unavailable`; it does not prevent `prnsd` from running.

Native service packages and other deliberately headless builds can omit the UI
dependencies:

```sh
cargo build --manifest-path prnsd/Cargo.toml --release --no-default-features \
  --features tokio-host,observability
```

The official container uses the narrower `tokio-cloud-host` profile, mandatory startup persistence, and a digest-pinned multi-architecture image. Consumer Docker and Railway operation is documented in [`docs/deploy-prnsd.md`](deploy-prnsd.md); image production, staging, template publication, and qualification are release responsibilities documented below.

The unified suite retains the flasher's established physical-acceptance
boundary. The protected suite public review, signed physical acceptance and
flasher release record, and protected deployment qualification remain
independent gates; stable promotion verifies all of them before moving the
GitHub Release or GHCR tags.

The default state directories are:

- Linux: `${XDG_STATE_HOME:-~/.local/state}/prnsd`
- macOS: `~/Library/Application Support/prnsd`
- Windows: `%LOCALAPPDATA%\prnsd`

The directory holds the versioned session record, readiness and shutdown
coordination, human and JSON logs, and one rotated predecessor for each log.
On Windows it also holds the managed launch copy, allowing the source executable
to be replaced while the daemon is running. Its files are private to the current
user under the platform's normal permissions.

This portable session survives the launching terminal, but it is not a
boot/login service and does not restart itself after a machine reboot or a
crash. Native service definitions should invoke `prnsd run --service --config
DIR` as their foreground process rather than nesting this session manager. The
[deployment guide](deploy-prnsd.md#systemd) provides a hardened systemd unit.

## Release the daemon and container

The active native artifact matrix is:

| Artifact | Platform |
| --- | --- |
| `prnsd-0.3.6-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64, glibc |
| `prnsd-0.3.6-aarch64-unknown-linux-gnu.tar.gz` | Linux ARM64, glibc |
| `prnsd-0.3.6-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `prnsd-0.3.6-aarch64-apple-darwin.tar.gz` | macOS Apple silicon |
| `prnsd-0.3.6-x86_64-pc-windows-msvc.zip` | Windows x86_64 |

Native archives contain the executable, licenses, third-party notices, Minisign public key, exact build identity, and the commit-bound `source.zip` plus its SHA-256 sidecar. Linux binaries are built natively on Ubuntu 24.04, making glibc 2.39 the supported baseline for this release. The full Linux build statically vendors its `libdbus` code. Every platform archive carries its complete linkage or import report as a signed-inventory asset.

The signed `prnsd-image-v0.3.6.json` asset binds the multi-platform OCI digest to the suite version, source commit, and amd64 and ARM64 child digests. `railway-template-contract-v0.3.6.json` binds the Railway publication to that exact image.

### Public staging

The public staging lane exercises the real container and Railway journey without creating a release, tag, signature, prerelease, or promotion record. It republishes exact reproducible OCI layouts under `ghcr.io/kenakafrosty/prnsd-staging`; the separate package name and generated metadata identify the artifact as staging while the executable bytes remain eligible to become a later release candidate.

Start from an exact protected `main` commit. Dispatch `prnsd-image-candidate.yml` with that full commit SHA and wait for its primary and reproduction builds to agree for amd64 and ARM64. Then dispatch `prnsd-staging-publish.yml` with the successful image-candidate run ID and leave `package_is_public` false on the first run. The workflow verifies the producer run, rechecks the OCI layouts and embedded source, preserves the platform manifest digests while copying them, and publishes only immutable `candidate-COMMIT` tags to the staging package.

GitHub creates a new container package as private. Change `prnsd-staging` to public only after confirming that this separate package is the intended permanent public staging surface; GitHub does not permit a public package to become private again. Rerun `prnsd-staging-publish.yml` with the same image-candidate run ID and `package_is_public` true. The workflow then proves anonymous digest access and uploads `prnsd-staging-image-COMMIT.json` plus `railway-staging-contract-COMMIT.json`.

The equivalent CLI dispatches are:

```sh
export SOURCE_COMMIT='FULL_PROTECTED_MAIN_SHA'
export IMAGE_CANDIDATE_RUN_ID='SUCCESSFUL_IMAGE_CANDIDATE_RUN_ID'

gh workflow run prnsd-image-candidate.yml \
  --ref main \
  -f commit_sha="$SOURCE_COMMIT"

gh workflow run prnsd-staging-publish.yml \
  --ref main \
  -f image_candidate_run_id="$IMAGE_CANDIDATE_RUN_ID" \
  -f package_is_public=false

gh workflow run prnsd-staging-publish.yml \
  --ref main \
  -f image_candidate_run_id="$IMAGE_CANDIDATE_RUN_ID" \
  -f package_is_public=true
```

Create a private Railway project from the exact `ghcr.io/kenakafrosty/prnsd-staging@sha256:...` reference in the generated contract. Apply the one-volume, one-replica, TCP-port-4242, restart-on-failure shape recorded there; do not override the image entrypoint or configure an HTTP-path health check. Set the fixed service variable `RAILWAY_RUN_UID=0` because Railway mounts volumes as root while the image normally runs as non-root UID 65532.

Railway's dashboard and template composer stage the service, volume, variables, service domain, and TCP proxy together. When provisioning sequentially, create the image service with `PRNSD_BACKBONE_DISCOVERABLE=Yes` and `RAILWAY_RUN_UID=0`, attach the volume, create the TCP proxy, wait for it to become active, and redeploy the same exact digest. The first start may fail closed while Railway's endpoint variables are incomplete; that prevents bootstrap from committing a permanently undiscoverable or invalid operator configuration.

After the service is healthy, capture its transport identity from the running container, exercise public Backbone and WebSocket connections, restart it, and confirm that the same identity and persisted routing state return:

```sh
railway ssh -- \
  /usr/local/bin/prnsd status \
  --config /var/lib/prnsd \
  --json
```

Publish an intentional second Railway template revision, roll back to the previous revision, and then restore the exact digest under test. Dispatch `prnsd-staging-qualification.yml` with the public endpoint, both observed identity hashes, both template revisions, the public staging publication run ID, and the required confirmations. That workflow anonymously pulls both architectures, checks the live TCP endpoint, verifies the staging publication chain, and emits staging-only evidence.

Staging evidence is never accepted by suite promotion. After the final source commit settles, release readiness and every producer workflow run again for that exact SHA; the signed `ghcr.io/kenakafrosty/prnsd` candidate and protected release deployment qualification remain separate authorities.

### Publish the Railway template

Railway's template composer is the publication authority for the Docker-image template. Publish the configuration recorded by the release's signed Railway contract:

1. Use `ghcr.io/kenakafrosty/prnsd@sha256:...` from the signed image metadata, never a mutable tag.

2. Set fixed service variable `RAILWAY_RUN_UID=0` and mount one persistent volume at `/var/lib/prnsd`.

3. Configure a TCP Proxy targeting internal port `4242` and a public service domain targeting internal port `4284`.

4. Run exactly one replica with restart-on-failure, JSON logging, no custom start command, and no HTTP-path health check.

5. Expose `PRNSD_BACKBONE_DISCOVERABLE`, `PRNSD_NNPAGES_ANNOUNCE`, and `PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES` as the small operator-owned set of template inputs.

6. Publish a new template revision for each intentional image upgrade instead of mutating a prior release revision.

Before stable promotion, the protected qualification workflow requires a private deployment of the precise template revision, successful public Backbone and WebSocket connections, persistence restoration with the same identity after restart, and an exercised rollback revision. Making the stable GHCR package public is also an explicit first-publication gate; both architectures must be anonymously pullable.

### Verify release artifacts

The release checksum inventory and record are verified with the repository's Minisign trust root:

```sh
minisign -Vm SHA256SUMS.txt \
  -x SHA256SUMS.txt.minisig \
  -p minisign.pub
minisign -Vm release-record-v0.3.6.json \
  -x release-record-v0.3.6.json.minisig \
  -p minisign.pub
sha256sum --check SHA256SUMS.txt
```

On macOS, use `shasum -a 256 -c SHA256SUMS.txt`. On Windows, compare
`(Get-FileHash ARCHIVE -Algorithm SHA256).Hash` against the matching
`SHA256SUMS.txt` entry. The release record binds native archives, signed flasher candidate, source and image SPDX SBOMs, image and platform digests, linkage reports, and GitHub provenance bundles into the exact checksum inventory.

The unified prerelease passes two protected evidence tracks before stable promotion. Physical flasher qualification adds `qualification-evidence-v0.3.6.tar.gz`, a signed acceptance document, and `flasher-release-record-v0.3.6.json`. Railway qualification adds `deployment-qualification-v0.3.6.json`. Promotion accepts only these narrowly named supplements and independently reverifies workflow custody, Minisign signatures, exact source, artifact digests, and live GitHub attestations.

```sh
gh attestation verify prnsd-0.3.6-x86_64-unknown-linux-gnu.tar.gz \
  --repo KenAKAFrosty/Prns
gh attestation verify \
  oci://ghcr.io/kenakafrosty/prnsd@sha256:REPLACE_WITH_SIGNED_DIGEST \
  --repo KenAKAFrosty/Prns
```

The suite uses the existing Minisign trust root and GitHub provenance. macOS notarization and Windows Authenticode are not present in `v0.3.6`; the archives are not platform-vendor-signed.

## Pre-1.0 semver

Cargo treats compatibility for `0.y.z` releases around the left-most non-zero
component. Prns follows that convention:

- `0.1.z` means compatible fixes or additive, low-risk public API changes.
- `0.2.0`, `0.3.0`, and later `0.y.0` releases may break public Rust APIs,
  feature flags, wire-adjacent contracts, or host integration points.
- `0.0.z` is reserved for scratch/internal packages that should not carry a
  public compatibility promise.
- `1.0.0` waits until the core engine API, feature selection, daemon boundary,
  and published crate set are stable enough to support as boring defaults.

## Crates and artifacts

Crates keep explicit `version` fields in their own `Cargo.toml` files. The
suite version in `VERSION` should match the primary public crate release unless
there is a deliberate crate-specific release.

Flash artifacts use the same build version by default when their manifest entry
still says `version = "next"`. Release jobs may set `PRNS_FLASH_VERSION` when a
firmware artifact intentionally needs a different prerelease or patch version.

Keep `publish = false` on crates until the release checklist for that crate is
complete. The first public cargo publish should include an audited manifest,
README, license metadata, feature list, docs.rs behavior, and a tag that points
at the exact source snapshot displayed by the docs site.

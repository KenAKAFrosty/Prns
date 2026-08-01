# Deploy prnsd

Official `prnsd` releases provide one complete native archive for each supported
desktop platform and one cloud-oriented Linux container image. Both products run
the same daemon. The image merely compiles out tray and locally attached-device
interfaces that are not useful inside a container.

The `v0.3.1` release matrix is:

| Artifact | Platform |
| --- | --- |
| `prnsd-0.3.1-x86_64-unknown-linux-gnu.tar.gz` | Linux x86_64, glibc |
| `prnsd-0.3.1-aarch64-unknown-linux-gnu.tar.gz` | Linux ARM64, glibc |
| `prnsd-0.3.1-x86_64-apple-darwin.tar.gz` | macOS Intel |
| `prnsd-0.3.1-aarch64-apple-darwin.tar.gz` | macOS Apple silicon |
| `prnsd-0.3.1-x86_64-pc-windows-msvc.zip` | Windows x86_64 |

Native archives contain the executable, licenses, third-party notices, Minisign
public key, exact build identity, and the commit-bound `source.zip` plus its
SHA-256 sidecar. The Linux binaries are built natively on
Ubuntu 24.04, so glibc 2.39 or newer is the supported baseline for this release.
The full Linux build statically vendors its `libdbus` code; it does not require
a separately installed `libdbus-1` shared library. Each release publishes the
complete `ldd`/`readelf`, `otool -L`, or PE import report as a signed-inventory
asset rather than relying on an undocumented portability claim.

## Run the container

Use an exact digest in production. The signed
`prnsd-image-v0.3.1.json` release asset binds the multi-platform digest to the
suite version, source commit, and amd64/ARM64 child digests.

```sh
export PRNSD_IMAGE='ghcr.io/kenakafrosty/prnsd@sha256:REPLACE_WITH_SIGNED_DIGEST'
docker pull "$PRNSD_IMAGE"
docker volume create prnsd-data
docker run -d \
  --name prnsd \
  --restart on-failure \
  --mount type=volume,source=prnsd-data,target=/var/lib/prnsd \
  --publish 4242:4242/tcp \
  --publish 4284:4284/tcp \
  "$PRNSD_IMAGE"
```

The image runs as UID and GID `65532`. A new server configuration listens for
Backbone connections on container port `4242` and WebSocket connections on
container port `4284`; the conventional WebSocket endpoint is
`ws://HOST:4284/prns`. The daemon emits JSON logs and stores configuration,
identity, routing state, ratchets, and the control endpoint under
`/var/lib/prnsd`. That path is not optional: an unwritable or missing persistent
mount makes startup fail instead of silently using ephemeral state.

For a public browser-facing deployment, terminate TLS at the platform or reverse
proxy and forward its `wss://HOST/prns` route to the container's plain WebSocket
listener on port `4284`.

For a bind mount, create and assign the directory before starting the container:

```sh
install -d -m 0700 ./prnsd-data
sudo chown 65532:65532 ./prnsd-data
```

Inspect readiness and logs without adding an HTTP service:

```sh
docker inspect --format '{{json .State.Health}}' prnsd
docker exec prnsd prnsd status --json
docker exec prnsd prnsd interfaces list
docker logs --follow prnsd
```

The entrypoint publishes `/var/lib/prnsd` as the container's active Prns
instance. Every `docker exec prnsd prnsd ...` operator command therefore uses
that configuration automatically. Running bare `prnsd` reports the already
running daemon instead of starting a second instance or falling back to a
home-directory configuration. An explicitly isolated second foreground
instance still requires its own `--config DIR` and state boundary.

Docker owns this foreground service's log attachment and restart lifecycle, so
use `docker logs` and `docker restart` rather than `prnsd logs` or
`prnsd restart` inside the container.

Docker sends `SIGTERM` because the image declares it as its stop signal. A
successful stop waits for acknowledged final routing-state and ratchet flushes:

```sh
docker stop --time 30 prnsd
```

The image always uses `--persistence-policy required`. Store initialization
failure, an unexpected persistence worker exit, or a failed write therefore
ends the process with a nonzero status. Conventional desktop and manually
configured daemon runs remain `best-effort` unless the operator chooses
`--persistence-policy required`.

## One-time server bootstrap

The image starts with:

```text
prnsd run --service --config /var/lib/prnsd --persistence-policy required --bootstrap server --log-format json
```

When `/var/lib/prnsd/config` does not exist, `--bootstrap server` atomically
creates a private `0600` configuration with one Backbone listener on
`0.0.0.0:4242`. After that first write the configuration is operator-owned:
bootstrap never rewrites an existing file.

The same first bootstrap creates `/var/lib/prnsd/nnpages`, with hosted
documents under `pages/`, downloads under `files/`, announcement policy in
`settings.toml`, and the optional display name in `name`. Every safe `.mu` path
below `nnpages/pages/` is published at `/page/<path>` on the node's
`nomadnetwork.node` destination; subdirectories become path segments. Regular
downloads below `nnpages/files/` are published at `/file/<path>`. Official
images stage their exact commit-bound source archive and checksum there, and
the seeded `source.mu` links to them without requiring internet access. File
handles are opened beneath those roots on each request and streamed one bounded
segment at a time, so edits and deletions take effect without retaining complete
downloads in memory.
Deleting `index.mu` also stops the node-page announcement and is never a daemon
error. Bootstrap does not recreate a deleted page once `config` exists.
The hosted page destination announcement is enabled by default and repeats
every 360 minutes, the conventional six-hour NomadNet cadence. Operators can
set `announce = false` or change `announce_interval_minutes` in
`/var/lib/prnsd/nnpages/settings.toml`; serving remains available when
announcement is disabled.

The daemon lightly reconciles filenames every five minutes. To publish an
addition, deletion, or rename immediately, run:

```sh
docker exec prnsd prnsd nnpages refresh
```

That command uses the configuration-local control lane, so it works with both
the foreground container daemon and a conventional managed installation.
Hidden and unsafe path components, symlinks, non-`.mu` files in
`nnpages/pages/`, pages larger than 1 MiB, and downloads larger than 32 MiB are
not served. Keep the NNPages tree private and edit it as UID/GID `65532` in the
container. The initially
seeded index is a gentle Prns introduction, not immutable product content:
operators may replace it or remove it to disable the showcase entirely.
`source.mu` and `coming-from-rns.mu` retain a first-line managed marker and are
refreshed by `prnsd nnpages seed` only while that marker remains; removing the
marker makes either page operator-owned. `prnsd nnpages seed --source` stages
the source bundle shipped beside a native executable or inside the image,
while `--source-archive /path/to/source.zip` supplies an explicit archive.
Existing different downloads are never overwritten.

The bootstrap environment is fail-closed:

- `PRNSD_BACKBONE_LISTEN_PORT` changes the internal listener port; its default
  is `4242`.
- `PRNSD_BACKBONE_DISCOVERABLE=No` suppresses Backbone discovery publication
  even when a complete public endpoint is present. `Yes` requires a complete
  endpoint. When omitted, publication is automatic only when that endpoint is
  complete.
- A complete `PRNSD_REACHABLE_HOST` and `PRNSD_REACHABLE_PORT` pair publishes
  the external discovery endpoint.
- Otherwise, a complete `RAILWAY_TCP_PROXY_DOMAIN` and
  `RAILWAY_TCP_PROXY_PORT` pair publishes Railway's endpoint.
- A generic `PRNSD_REACHABLE_*` pair takes precedence over Railway variables.
- Partial pairs, zero or invalid ports, malformed hosts, and conflicting
  partial input stop startup.
- Discovery stays disabled when there is no complete published endpoint.
- `PRNSD_NNPAGES_ANNOUNCE=No` disables the seeded page's announcement while
  continuing to serve it by destination hash.
- `PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES` selects a positive whole number
  of minutes and defaults to `360`.

The published port may differ from the listener port. The generated
`reachable_port` is advertised to peers while `listen_port` remains the local
socket. Operators can make the same distinction in a hand-written listening
Backbone or TCP server stanza.

To change bootstrap inputs after creation, edit the Reticulum `config` or
NNPages `settings.toml` deliberately; restarting with different environment
variables does not mutate either operator-owned file or recreate deleted pages.

For a running container, Backbone publication is an ordinary interface setting
and can be changed without a restart:

```sh
docker exec prnsd prnsd interfaces edit "Cloud Backbone" \
  --discoverable false \
  --apply
```

NNPages policy is independent of the stock Reticulum configuration. Edit the
sidecar and ask the daemon to reload it:

```toml
# /var/lib/prnsd/nnpages/settings.toml
announce = false
announce_interval_minutes = 360
```

```sh
docker exec prnsd prnsd nnpages refresh
```

## Backup and restore

Stop the daemon first so the backup represents one acknowledged final flush.
The released image itself can act as a shell-free volume carrier:

```sh
docker stop --time 30 prnsd
mkdir -p ./prnsd-backup
docker create \
  --name prnsd-backup-carrier \
  --mount type=volume,source=prnsd-data,target=/var/lib/prnsd \
  "$PRNSD_IMAGE" status --config /var/lib/prnsd --json
docker cp -a prnsd-backup-carrier:/var/lib/prnsd/. ./prnsd-backup/
docker rm prnsd-backup-carrier
docker start prnsd
```

Keep the directory private: it contains the node identity and ratchets. To
restore, stop and remove the daemon, mount an empty replacement volume in a
carrier created from the desired image, copy the saved contents back with
archive mode, and then recreate the daemon:

```sh
docker stop --time 30 prnsd
docker rm prnsd
docker volume create prnsd-restored
docker create \
  --name prnsd-restore-carrier \
  --mount type=volume,source=prnsd-restored,target=/var/lib/prnsd \
  "$PRNSD_IMAGE" status --config /var/lib/prnsd --json
docker cp -a ./prnsd-backup/. prnsd-restore-carrier:/var/lib/prnsd/
docker rm prnsd-restore-carrier
```

Inspect that restored files remain owned by `65532:65532` on Linux before
starting the replacement. Never run two replicas against one copied identity or
one writable state directory.

## Upgrade and rollback

Back up first, then recreate the container at the newly verified digest while
retaining the volume:

```sh
export PRNSD_IMAGE='ghcr.io/kenakafrosty/prnsd@sha256:NEW_SIGNED_DIGEST'
docker pull "$PRNSD_IMAGE"
docker stop --time 30 prnsd
docker rm prnsd
docker run -d \
  --name prnsd \
  --restart on-failure \
  --mount type=volume,source=prnsd-data,target=/var/lib/prnsd \
  --publish 4242:4242/tcp \
  --publish 4284:4284/tcp \
  "$PRNSD_IMAGE"
```

Rollback uses the identical procedure with the previous verified digest and
the state backup appropriate to that version. Tags `0.3.1` and `latest` are
convenient discovery aliases, not deployment locks; promotion only moves them
after independent verification, but an exact digest remains the durable
operator contract.

## Railway

Railway's template composer, rather than a repository file, is the publication
authority for a Docker-image template. The signed
`railway-template-contract-v0.3.1.json` release asset records the exact settings
that must be published:

1. Use `ghcr.io/kenakafrosty/prnsd@sha256:...` from the signed image metadata,
   never a mutable tag.
2. Mount one persistent volume at `/var/lib/prnsd`.
3. Configure one TCP Proxy targeting internal port `4242`.
4. Run exactly one replica and select restart-on-failure.
5. Keep JSON logging and do not configure an HTTP-path health check.
6. Publish a new template revision for an intentional image upgrade rather
   than mutating the old revision.

Railway supplies `RAILWAY_TCP_PROXY_DOMAIN` and
`RAILWAY_TCP_PROXY_PORT`. The one-time bootstrap therefore advertises the
public proxy port while continuing to listen on `4242` inside the service.
Expose `PRNSD_BACKBONE_DISCOVERABLE`, `PRNSD_NNPAGES_ANNOUNCE`, and
`PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES` as template variables so the initial
operator-owned Reticulum configuration and NNPages sidecar are
deployment-controllable.

Before stable promotion, the protected qualification workflow requires a
private deployment of the precise template revision, a successful public
Backbone connection, persistence restoration with the same identity after a
restart, and an exercised rollback revision. It records those facts as
release-bound evidence. Making the GHCR package public is also an explicit
first-publication gate; both architectures must be anonymously pullable.

## Verify a release

Download the assets from the exact GitHub release tag. Trust starts with the
repository's `release/keys/minisign.pub`; compare it through an independent
channel before first use.

```sh
minisign -Vm SHA256SUMS.txt \
  -x SHA256SUMS.txt.minisig \
  -p minisign.pub
minisign -Vm release-record-v0.3.1.json \
  -x release-record-v0.3.1.json.minisig \
  -p minisign.pub
sha256sum --check SHA256SUMS.txt
```

On macOS, use `shasum -a 256 -c SHA256SUMS.txt`. The release record binds the
native archives, signed flasher candidate, source and image SPDX SBOMs, image
and platform digests, linkage reports, and GitHub provenance bundles into that
exact checksum inventory.

The unified prerelease then passes two protected evidence tracks before stable
promotion. Its public-review job reviews the visible suite without access to
the signing secret. Physical flasher installation and device qualification add
`qualification-evidence-v0.3.1.tar.gz`, a signed acceptance document, and a
separately signed `flasher-release-record-v0.3.1.json`. Railway qualification
adds `deployment-qualification-v0.3.1.json` after its independently supplied
digest is verified. These post-publication evidence files are narrowly named
supplements to the immutable suite inventory; promotion rejects every other
uninventoried asset and independently reverifies their workflow custody,
Minisign signatures, exact source, artifact digests, and live GitHub
attestations.

Verify an archive and the immutable OCI subject against GitHub's provenance:

```sh
gh attestation verify prnsd-0.3.1-x86_64-unknown-linux-gnu.tar.gz \
  --repo KenAKAFrosty/Prns
gh attestation verify \
  oci://ghcr.io/kenakafrosty/prnsd@sha256:REPLACE_WITH_SIGNED_DIGEST \
  --repo KenAKAFrosty/Prns
```

SPDX files are ordinary JSON and can be inspected without special tooling:

```sh
jq '.creationInfo, .packages[] | {name, versionInfo, licenseConcluded}' \
  prnsd-0.3.1-source.spdx.json
```

The suite uses the existing Minisign trust root and GitHub provenance. macOS
notarization and Windows Authenticode are not present in `v0.3.1`; do not treat
the archives as platform-vendor-signed.

## Host feature profiles and persistence events

`tokio-host` remains the complete Tokio host-platform profile: cloud transports,
tray-capable desktop operation, serial, KISS, AX.25, RNode, Weave, Wi-Fi auto,
USB, and Bluetooth auto. `tokio-cloud-host` is its cloud-oriented variation. It
retains configuration, persistence, shared-instance support, TCP, UDP, pipes,
Backbone, I2P, WebSocket, browser rendezvous, signed artifacts, RNX, and
parallel work while excluding tray and locally attached-device capabilities.
Embassy remains the embedded host platform and does not inherit either Tokio
profile.

Persistence event layering is intentional. The lower-level host worker injects
`Journaled::PersistenceFlushed` and `Journaled::PersistenceFlushFailed` into the
ordered engine journal because those notifications must retain their position
relative to engine work even though the engine performs no storage I/O.
Recipe-managed restoration and terminal persistence notifications still travel
through the normal manifold/application event path and its panic boundary.
Shutdown acknowledgement therefore means the application observed the terminal
success or failure before the lifecycle future returned; `Journaled` is an
ordering transport, not a bypass around application events.

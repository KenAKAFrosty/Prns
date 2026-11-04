# Deploy prnsd

`prnsd` can run as a native systemd service or in the official container, including on [Fly.io](#flyio) and [Railway](#railway). Each path runs the same foreground daemon with persistent configuration, transport identity, routing state, ratchets, and pages.

## systemd

Install the release binary and create a dedicated service account:

```sh
sudo install -Dm0755 ./prnsd /usr/local/bin/prnsd
sudo useradd --system --user-group --home-dir /var/lib/prnsd --shell /usr/sbin/nologin prnsd
```

Save the following unit as `/etc/systemd/system/prnsd.service`:

```ini
[Unit]
Description=Prns daemon
After=network.target

[Service]
Type=simple
User=prnsd
Group=prnsd

StateDirectory=prnsd
StateDirectoryMode=0700
RuntimeDirectory=prnsd
RuntimeDirectoryMode=0700
Environment=PRNSD_STATE_DIR=/run/prnsd

ExecStart=/usr/local/bin/prnsd run --service --config /var/lib/prnsd --persistence-policy required --log-format json
Restart=on-failure
RestartSec=5s
TimeoutStopSec=30s
UMask=0077

NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true

[Install]
WantedBy=multi-user.target
```

`StateDirectory` gives the service a persistent, private `/var/lib/prnsd`; `RuntimeDirectory` creates the private `/run/prnsd` control directory on each service start. `--service` registers the foreground process there so operator commands can find the routing owner, while systemd remains responsible for starting, stopping, and restarting it. Required persistence makes startup fail when the node cannot establish writable stable state, and JSON events go to journald.

Load and start the service:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now prnsd
sudo systemctl status prnsd
sudo journalctl --unit prnsd --follow
```

The runtime directory is private to the service account. Run operator commands as that account and point them at the same control state and configuration:

```sh
sudo -u prnsd env PRNSD_STATE_DIR=/run/prnsd \
  /usr/local/bin/prnsd status --config /var/lib/prnsd
sudo -u prnsd env PRNSD_STATE_DIR=/run/prnsd \
  /usr/local/bin/prnsd interfaces list --config /var/lib/prnsd
```

Use `systemctl restart prnsd` and `systemctl stop prnsd` for lifecycle operations. SIGTERM enters the daemon's graceful shutdown path, including the final persistence flush. Hardware interfaces may require distribution-specific device permissions for the `prnsd` account; the unit intentionally does not grant broad device access.

## Docker

### Start the container

Create a named volume and publish the default Reticulum TCP and WebSocket ports:

```sh
docker volume create prnsd-data

docker run -d \
  --name prnsd \
  --restart unless-stopped \
  --mount type=volume,source=prnsd-data,target=/var/lib/prnsd \
  --publish 4242:4242/tcp \
  --publish 4284:4284/tcp \
  ghcr.io/kenakafrosty/prnsd:0.3.8
```

The `unless-stopped` policy restarts the node with Docker after a host reboot, and still honors a manual `docker stop`.

Reticulum TCP clients connect to `HOST:4242`. WebSocket clients connect to `ws://HOST:4284/prns`; browsers served over HTTPS require a certificate-valid `wss://` endpoint, normally supplied by your hosting platform or reverse proxy. A free tunnel service like Cloudflare Tunnel supplies one from a home machine too, with no port forwarding and no exposed home IP.

The image supports amd64 and ARM64 Linux. [Browse the official image on GHCR](https://github.com/KenAKAFrosty/Prns/pkgs/container/prnsd).

### Advertise a public Backbone endpoint

The container cannot infer the public address and port created by arbitrary Docker hosts, NAT, or port forwarding. A directly hosted public node can publish its reachable endpoint during first startup:

```sh
docker run -d \
  --name prnsd \
  --restart unless-stopped \
  --mount type=volume,source=prnsd-data,target=/var/lib/prnsd \
  --publish 4242:4242/tcp \
  --publish 4284:4284/tcp \
  --env PRNSD_BACKBONE_DISCOVERABLE=Yes \
  --env PRNSD_REACHABLE_HOST=backbone.example.com \
  --env PRNSD_REACHABLE_PORT=4242 \
  ghcr.io/kenakafrosty/prnsd:0.3.8
```

`PRNSD_REACHABLE_PORT` is the external port that other nodes use; it may differ from the container's internal listener port. Discovery remains disabled when no complete public endpoint is available. Partial or malformed endpoint settings fail startup instead of creating an invalid advertisement.

Bootstrap settings are used only when `/var/lib/prnsd/config` does not exist. After that first start, the configuration belongs to you and environment changes do not silently rewrite it. Use `prnsd interfaces` to change a running node.

### Operate it

```sh
docker inspect --format '{{json .State.Health}}' prnsd
docker exec prnsd prnsd status --json
docker exec prnsd prnsd interfaces list
docker logs --follow prnsd
```

Open the guided interface editor or a normal Debian shell when you want to work interactively:

```sh
docker exec -it prnsd prnsd interfaces
docker exec -it prnsd /bin/sh
```

The image normally runs as non-root UID and GID `65532`. Root is available for exceptional diagnostics through `docker exec -u 0 -it prnsd /bin/sh`; packages installed interactively disappear when the container is replaced.

Docker owns this foreground process, so use `docker logs`, `docker restart`, and `docker stop` rather than the managed-desktop lifecycle commands. A graceful stop waits for final routing-state and ratchet writes:

```sh
docker stop --time 30 prnsd
```

The container fails closed when it cannot establish writable persistent storage or its stable transport identity during startup. If a later state or ratchet write fails, the daemon reports the failure and keeps routing from memory. Routing-state writes retry after later changes and at the periodic flush; ratchets remain live in memory and receive another full write during graceful shutdown. State learned after the last successful write can be lost if the instance stops before storage recovers. A failed final shutdown write still returns a nonzero process status.

### Configuration and hosted pages

A new volume receives a private server configuration with a Backbone listener on `0.0.0.0:4242` and a WebSocket listener on `0.0.0.0:4284`. It also receives an editable NNPages tree:

```text
/var/lib/prnsd/nnpages/
├── files/
├── pages/
├── name
└── settings.toml
```

Place `.mu` pages under `pages/` and downloads under `files/`. Changes are discovered periodically; apply them immediately with:

```sh
docker exec prnsd prnsd nnpages refresh
```

The most useful first-start settings are:

| Variable | Purpose |
| --- | --- |
| `PRNSD_BACKBONE_LISTEN_PORT` | Change the internal Backbone listener from `4242` |
| `PRNSD_REACHABLE_HOST` and `PRNSD_REACHABLE_PORT` | Publish a complete external Backbone endpoint |
| `PRNSD_BACKBONE_DISCOVERABLE` | Explicitly enable or disable Backbone discovery |
| `PRNSD_NNPAGES_ANNOUNCE` | Enable or disable the hosted page announcement |
| `PRNSD_NNPAGES_ANNOUNCE_INTERVAL_MINUTES` | Change the default six-hour announcement interval |

The [configuration reference](prnsd-config.md) covers every interface and routing setting. [Observability](observability.md) covers structured logs, the shipped dashboard, and optional OTLP/HTTP metrics and traces.

### Back up and restore

Stop the daemon first so the backup includes an acknowledged final flush:

```sh
docker stop --time 30 prnsd
mkdir -p ./prnsd-backup
docker cp -a prnsd:/var/lib/prnsd/. ./prnsd-backup/
docker start prnsd
```

Keep the backup private because it contains the node identity and ratchets. Restore it only into a stopped container or an empty replacement volume, preserve UID/GID `65532`, and never run two replicas from the same identity or writable state directory.

### Upgrade or roll back

Back up the volume, pull the desired release, and recreate the container while retaining the volume:

```sh
export PRNSD_IMAGE='ghcr.io/kenakafrosty/prnsd:0.3.8'
docker pull "$PRNSD_IMAGE"
docker stop --time 30 prnsd
docker rm prnsd
docker run -d \
  --name prnsd \
  --restart unless-stopped \
  --mount type=volume,source=prnsd-data,target=/var/lib/prnsd \
  --publish 4242:4242/tcp \
  --publish 4284:4284/tcp \
  "$PRNSD_IMAGE"
```

Rollback is the same operation with the previous release and its compatible state backup. Version tags are convenient for ordinary use; production operators who need an immutable deployment can replace the tag with the exact multi-platform digest published by the matching GitHub release.

## Fly.io

The repository's [`fly.toml`](../fly.toml) uses the existing Dockerfile to run one always-on node with persistent storage, public Reticulum TCP on port `4242`, and HTTPS for the `/prns` WebSocket endpoint. It defaults to one shared CPU, 1 GB RAM, and the `iad` region. If you change the region, keep the node and its volume in the same region.

### First-time setup

Install the [Fly CLI](https://docs.fly.io/flyctl/install/), then run these commands from the repository root. Choose a globally unique app name and an organization in the account you want to use (`fly orgs list` shows your organizations):

```sh
fly auth login
export PRNS_FLY_APP='your-unique-prns-node'
export PRNS_FLY_ORG='personal'
fly apps create "$PRNS_FLY_APP" --org "$PRNS_FLY_ORG"
fly ips allocate-v4 --app "$PRNS_FLY_APP"
```

The app and dedicated IPv4 are one-time setup. Plain Reticulum TCP needs a [dedicated IPv4](https://docs.fly.io/networking/services#dedicated-ipv4); the shared HTTP/TLS address cannot route it. The app name is supplied with `--app`, so the checked-in configuration can be reused across accounts. An optional `fly ips allocate-v6 --app "$PRNS_FLY_APP"` also enables public IPv6.

The `mounts.initial_size` setting creates the first `prnsd_data` volume automatically during deployment. Keep this volume through updates: it holds the node identity, configuration, routing state, ratchets, and hosted pages.

### Build and deploy

Run this command for the first deployment and each source update, using a clean checkout of the revision you intend to deploy:

```sh
fly deploy --app "$PRNS_FLY_APP" --ha=false \
  --env "PRNSD_REACHABLE_HOST=${PRNS_FLY_APP}.fly.dev" \
  --build-arg "VERSION=$(cat VERSION)" \
  --build-arg "VCS_REF=$(git rev-parse HEAD)" \
  --build-arg "SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD)" \
  --build-arg "BUILD_DATE=$(git show -s --format=%cI HEAD)"
```

The four build arguments are required by the existing Dockerfile; the source revision supplies the version and timestamps so an unchanged checkout keeps the same build metadata. Fly builds remotely by default. To deploy a previously built release instead, replace the four `--build-arg` options with `--image ghcr.io/kenakafrosty/prnsd@sha256:REPLACE_WITH_RELEASE_DIGEST`; keep the app, hostname, and `--ha=false` options. [Verify the release image](#verify-an-exact-image) before using its digest.

`PRNSD_REACHABLE_HOST` is calculated by the shell because `fly.toml` does not expand environment variables. Together with the configured port, it seeds a new node's public Backbone advertisement. Existing persistent configuration is not rewritten on deployment; use `prnsd interfaces` to change it later.

The configuration disables automatic sleep, restarts the Machine after process exit, and gives the daemon a 30-second graceful shutdown allowance. Proxy autostart is disabled so an intentional maintenance stop stays stopped. A singleton rolling deployment interrupts connections briefly; clients must reconnect. `--ha=false` prevents adding default redundancy. Keep exactly one Machine for this node identity; it does not reduce an app that was already scaled out.

### Connect and inspect

In the phone app, save a TCP peer with host `YOUR_APP.fly.dev` and port `4242`. WebSocket clients use `wss://YOUR_APP.fly.dev/prns`.

```sh
fly status --app "$PRNS_FLY_APP"
fly ssh console --app "$PRNS_FLY_APP" --command '/usr/local/bin/prnsd status --config /var/lib/prnsd --json'
fly logs --app "$PRNS_FLY_APP"
```

The configured TCP check verifies the listener. Use the daemon status and actual client connections to verify routing. Before sharing an alpha endpoint, test messaging between phones on separate networks, restart the Machine, and confirm that the same transport identity returns and both clients reconnect.

### Storage and recovery

The container requires writable persistent storage and runs as UID/GID `65532`. If a first deployment reports a volume permission error, stop its failed Machine before attaching the volume to a temporary helper. For a fresh node that has never started successfully:

```sh
fly scale count 0 --app "$PRNS_FLY_APP"
fly machine run \
  debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 \
  --app "$PRNS_FLY_APP" --region iad \
  --volume prnsd_data:/var/lib/prnsd \
  --shell --user root --command /bin/sh
```

Inside the helper shell, run:

```sh
chown 65532:65532 /var/lib/prnsd
chmod 0700 /var/lib/prnsd
stat -c '%u:%g %a' /var/lib/prnsd
exit
```

This helper uses the Dockerfile's root-default Debian base, never starts PRNS, and removes itself after the shell exits. Confirm `stat` prints `65532:65532 700` and the volume is unattached (`fly volumes list --app "$PRNS_FLY_APP"`), then rerun the deploy command. Use the configured region instead of `iad` if you changed it. Do not run this first-start procedure against an existing active node.

Fly takes daily volume snapshots; the configuration retains them for 14 days. Fly volumes are local to a host and are not automatically replicated. Keep a separate private backup and test restoration; snapshots alone are not the primary backup plan. For an application-consistent backup, stop the daemon gracefully and confirm a successful final flush before copying or snapshotting its state. See [Fly volume management](https://docs.fly.io/volumes/volume-manage/) for snapshot restoration and volume attachment.

Restore into a stopped replacement and preserve UID/GID `65532`. Never start the original and restored node together. If the replacement has a different app name or public endpoint, update the persisted interface configuration before making it public; changing bootstrap environment variables does not update an existing config. Back up state before an image upgrade and retain the prior image digest plus its compatible backup for rollback.

## Railway

The repository includes Railway integration and a release-generated deployment contract for one `prnsd` service, one persistent `/var/lib/prnsd` volume, a public Reticulum TCP proxy, and a certificate-backed public WebSocket domain. The [0.3.8 deployment record](../release/deployment/0.3.8.json) records that no project or published template was available and hosted qualification was deferred. Do not assume a public template exists from this guide alone.

### Deploy and connect

Create a project from the exact release image and configuration described in the [Railway deployment instructions](release.md#publish-the-railway-template), or use a published template once its release revision is available. Wait for the service to become healthy, then open its **Networking** panel:

- Copy the **TCP Proxy** host and port into an ordinary Reticulum Backbone or TCP client.

- Copy the generated public domain and use `wss://YOUR-PUBLIC-DOMAIN/prns` in the Prns browser playground or another WebSocket client.

The generated domain and TCP proxy remain attached across normal deployments and restarts. Deleting and recreating either networking resource can assign a different endpoint.

Railway supplies its external TCP proxy address to the container, so first-start Backbone discovery can advertise the actual public host and assigned port while the daemon continues listening on internal port `4242`.

### Configure and inspect

The template exposes only the useful first-start choices: Backbone discovery, NNPages announcements, and their interval. As with Docker, those values create the initial operator-owned configuration and do not overwrite later edits.

Railway's browser console and `railway ssh` open the shell included in the image. The operator CLI works directly against the active service configuration:

```sh
prnsd status --json
prnsd interfaces list
prnsd interfaces
prnsd nnpages refresh
```

Keep one replica attached to the volume. Scaling a stateful transport identity horizontally would make multiple nodes claim the same identity and is not supported. Resize the volume if your actual routing state, pages, or downloads outgrow its initial capacity.

Use Railway's deployment logs and metrics for the basic service view. If you already operate an OpenTelemetry collector, point the optional OTLP settings at it using the environment variables in the [observability guide](observability.md).

### Back up and update

Back up the persistent volume before changing image versions, and confirm that the same transport identity returns after a restart or restore. A new template revision affects new deployments; an existing service remains under your control and can be moved deliberately to the exact image tag or digest from a newer release.

Do not remove the volume when redeploying. Do not run a rollback and the current service simultaneously from copied state.

## Verify an exact image

Each stable GitHub release publishes the multi-platform image digest and provenance beside the native archives. To lock a deployment, use the recorded `ghcr.io/kenakafrosty/prnsd@sha256:...` reference instead of a tag. Operators using the GitHub CLI can also verify the immutable OCI subject:

```sh
gh attestation verify \
  oci://ghcr.io/kenakafrosty/prnsd@sha256:REPLACE_WITH_RELEASE_DIGEST \
  --repo KenAKAFrosty/Prns
```

The [GitHub releases page](https://github.com/KenAKAFrosty/Prns/releases) is the authoritative source for a release's digest, checksums, signatures, and provenance.

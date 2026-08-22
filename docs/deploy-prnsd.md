# Deploy prnsd

`prnsd` can run as a native systemd service or in the official container. Each path runs the same foreground daemon with persistent configuration, transport identity, routing state, ratchets, and pages.

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
  ghcr.io/kenakafrosty/prnsd:0.3.7
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
  ghcr.io/kenakafrosty/prnsd:0.3.7
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
export PRNSD_IMAGE='ghcr.io/kenakafrosty/prnsd:0.3.7'
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

## Railway

The public Prns Railway template is the shortest path to a hosted backbone when you do not want to manage Docker or a server. It creates one `prnsd` service, one persistent `/var/lib/prnsd` volume, a public Reticulum TCP proxy, and a certificate-backed public WebSocket domain.

### Deploy and connect

Deploy the template into your Railway account and wait for the service to become healthy. Then open the service's **Networking** panel:

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

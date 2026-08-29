# prnsd

`prnsd` is the Prns daemon: one binary that runs a high-performance Reticulum transport node, shares it with every Reticulum app on the machine, and carries its own operator toolkit.

If you run `rnsd` today, this is its replacement. Your config, your identity file, and your apps carry over unchanged; [the full before-and-after is here](../docs/coming-from-rns.md).

## Start it

Download the [latest release](https://github.com/KenAKAFrosty/Prns/releases) for your platform, unpack the archive, and run the binary:

```console
prnsd
```

The `v0.3.7` archives are not platform-vendor-signed, so the first launch of a downloaded binary may trigger Windows SmartScreen. Do **More info → Run anyway**. On macOS, Gatekeeper wants a right-click **Open** the first time. Every release is checksummed, Minisign-signed, and GitHub-attested instead. [Verify release artifacts](../docs/release.md#verify-release-artifacts) walks the whole chain, including the Windows `Get-FileHash` comparison against `SHA256SUMS.txt`. If security software later asks about `prnsd-managed.exe`, that is the daemon's own staged copy of its binary, part of the managed lifecycle.

It starts with your existing Reticulum configuration from the standard location, and writes its built-in one there first if the machine has never had one. Running `prnsd` again attaches to the daemon already running, and Ctrl-C detaches without stopping it. Three more verbs round out the lifecycle:

```console
prnsd logs
prnsd restart
prnsd stop
```

On a desktop the daemon also sits in the system tray with a live status readout and one-click access to the interface editor, the configuration folder, and stop. Headless machines skip the tray and run on undisturbed.

## Point your apps at it

`prnsd` is the machine's shared instance: Sideband, NomadNet, and the rest of the RNS app ecosystem connect to it exactly as they connect to `rnsd` today. When another Reticulum instance already owns the shared-instance role, `prnsd` joins it as a client instead of competing for it (though note this would be an abnormal and suboptimal configuration).

## Keep your config

`prnsd` reads the stock RNS config file format from the standard locations; pass `--config DIR` to select `DIR/config` instead. A broken config produces a diagnostic that names the line, what it found, what it accepts, and the fix to apply.

`prnsd interfaces` opens an interactive editor for that same file, and every verb in it is also scriptable: `list`, `validate`, `add`, `edit`, `enable`, `disable`, `remove`, `repair`, and `apply`. Changes print a diff, then save atomically with a backup of the previous file; comments and unrelated settings stay put. `apply` hands the change to the running daemon, which reconciles interfaces in place with no restart. The [Prnsd configuration reference](../docs/prnsd-config.md) more densely covers the editor, scripting, repair, and remote management end to end.

## Operate it

The daemon binary is also the utility toolkit. `prnsd status`, `path`, `probe`, `id`, `cp`, and `x` are the equivalents of the stock `rn*` utilities, with secure defaults: `cp --listen` and `x --listen` permit nobody until an identity is allowed, and remote management stays off until you enable it. [Prnsd utilities](../docs/prnsd-utilities.md) documents each role.

Every log line is a structured event: human-readable by default, the same events as JSON with `--log-format json`, and a rotated pair of log files either way. The official cloud container and canonical `cargo prnsd build` artifact also carry an OTLP exporter for metrics and traces. [Observability](../docs/observability.md) goes from log filters to the shipped Grafana dashboard.

For I2P interfaces, `prnsd i2p doctor` checks your SAM bridge and `prnsd i2p setup` walks the setup.

## Host NomadNet pages

The daemon that owns your routing tables can host your NomadNet pages too. Drop `.mu` files into `nnpages/pages/` under the active configuration directory and they serve from the node's `nomadnetwork.node` destination; `nnpages/files/` serves downloads. Edits are read live, path additions and removals reconcile every five minutes, and `prnsd nnpages` carries the CLI surface: `seed` lays down the editable layout, `refresh` reconciles immediately, `announce` announces on demand, and `rename "My Node"` sets the display name. Source hosting is opt-in with `nnpages seed --source`.

These commands target the active managed or service-owned daemon configuration automatically, then fall back to the normal platform Reticulum directory when no daemon is active. The official container entrypoint publishes `/var/lib/prnsd` as that active context, so `docker exec prnsd prnsd nnpages refresh` needs no path incantation. A deliberately isolated raw foreground run still selects its own `--config DIR`. [The pages section of the before-and-after](../docs/coming-from-rns.md#serve-nomadnet-pages-directly-from-the-daemon) tells the full story.

## Deploy it

Official releases ship one native archive per desktop platform and a cloud-oriented container image, all running the same daemon.

### Docker

If Docker is already part of your toolkit, this starts one persistent Prns backbone from the official `0.3.7` image:

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

Reticulum TCP clients can connect to `HOST:4242`, and WebSocket clients can connect to `ws://HOST:4284/prns`. Public browser deployments need a certificate-valid `wss://` endpoint, normally supplied by the hosting platform or a reverse proxy.

[Browse the image on GHCR](https://github.com/KenAKAFrosty/Prns/pkgs/container/prnsd). The complete [deployment guide](../docs/deploy-prnsd.md) covers exact digest pinning, public discovery, inspection, backups, upgrades, and verification.

### Railway

If you would rather not manage Docker or a server yourself, the Railway template creates the same daemon in your Railway account with persistent storage, a public Reticulum TCP address, and a certificate-backed WebSocket address. The public template link is published with the exact release image revision instead of pointing new deployments at a mutable or unqualified image.

After deployment, open the service's **Networking** panel. Copy its **TCP Proxy** address for Reticulum clients; its browser address is `wss://YOUR-PUBLIC-DOMAIN/prns`. The [Railway deployment guide](../docs/deploy-prnsd.md#railway) covers the template's small set of controls and the underlying deployment shape.

## Work in the repository

From a clone, `cargo prnsd` builds the daemon and manages one per-user process with the same verbs; build options go before `--`, daemon options after it. Keep a walkthrough isolated from your real node with a separate state directory and config:

```console
export PRNSD_STATE_DIR="$PWD/target/quickstart-service"
./tools/prns doctor node
cargo prnsd --detach -- --config target/quickstart-node
cargo prnsd status --config target/quickstart-node
cargo prnsd stop
```

(PowerShell uses `$env:PRNSD_STATE_DIR="$PWD\target\quickstart-service"` and `.\tools\prns.cmd doctor node` in place of `./tools/prns doctor node`.)

If `target/quickstart-node/config` does not exist, `prnsd` materializes the built-in configuration under that isolated directory, with transport and the supported automatic interfaces enabled. `cargo prnsd build` produces the locked release-profile artifact, and `cargo prnsd -- --help` prints the daemon's direct options without starting a managed session.

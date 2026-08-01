# prnsd

`prnsd` is the Prns daemon: one binary that runs a high-performance Reticulum transport node, shares it with every Reticulum app on the machine, and carries its own operator toolkit.

If you run `rnsd` today, this is its replacement. Your config, your identity file, and your apps carry over unchanged; [the full before-and-after is here](../docs/coming-from-rns.md).

## Start it

Download the [latest release](https://github.com/KenAKAFrosty/Prns/releases) for your platform, unpack the archive, and run the binary:

```console
prnsd
```

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

The daemon that owns your routing tables can host your NomadNet pages too. Drop `.mu` files into `nnpages/pages/` under the active configuration directory and they serve from the node's `nomadnetwork.node` destination; `nnpages/files/` serves downloads. Edits are read live, path additions and removals reconcile every five minutes, and `prnsd nnpages` carries the CLI surface: `seed` lays down the complete editable layout, `refresh` reconciles immediately, `announce` announces on demand, and `rename "My Node"` sets the display name.

These commands target the active managed or service-owned daemon configuration automatically, then fall back to the normal platform Reticulum directory when no daemon is active. The official container entrypoint publishes `/var/lib/prnsd` as that active context, so `docker exec prnsd prnsd nnpages refresh` needs no path incantation. A deliberately isolated raw foreground run still selects its own `--config DIR`. [The pages section of the before-and-after](../docs/coming-from-rns.md#serve-nomadnet-pages-directly-from-the-daemon) tells the full story.

## Deploy it

Official releases ship one native archive per desktop platform and a cloud-oriented container image, all running the same daemon. [Deploy prnsd](../docs/deploy-prnsd.md) covers the signed releases, the container, one-time cloud bootstrap, Railway, backups, upgrades, and verification.

## Work in the repository

From a clone, `cargo prnsd` builds the daemon and manages one per-user process with the same verbs; build options go before `--`, daemon options after it. Keep a walkthrough isolated from your real node with a separate state directory and config:

```console
export PRNSD_STATE_DIR="$PWD/target/quickstart-service"
./tools/prns doctor node
cargo prnsd --debug --detach -- --config target/quickstart-node
cargo prnsd status --config target/quickstart-node
cargo prnsd stop
```

(PowerShell uses `$env:PRNSD_STATE_DIR="$PWD\target\quickstart-service"`.)

If `target/quickstart-node/config` does not exist, `prnsd` materializes the built-in configuration under that isolated directory, with transport and the supported automatic interfaces enabled. `cargo prnsd build` produces the locked release-profile artifact, and `cargo prnsd -- --help` prints the daemon's direct options without starting a managed session.

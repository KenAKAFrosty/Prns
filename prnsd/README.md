# Prnsd

`prnsd` is the native Prns daemon. It runs a Reticulum node, supports stock
Reticulum configuration and shared-instance clients, and provides compatible
utilities for status, paths, probes, identities, file transfer, and remote
execution. The repository-local `cargo prnsd` command builds it, manages one
per-user process, and exposes its inspection commands.

## Run an isolated node

Use a separate daemon state directory and Reticulum configuration so the
walkthrough neither reuses nor stops your normal managed node:

```console
export PRNSD_STATE_DIR="$PWD/target/quickstart-service"
./tools/prns doctor node
cargo prnsd --debug --detach -- --config target/quickstart-node
cargo prnsd interfaces --config target/quickstart-node list
cargo prnsd status --config target/quickstart-node
cargo prnsd stop
```

PowerShell uses:

```console
$env:PRNSD_STATE_DIR="$PWD\target\quickstart-service"
```

If `target/quickstart-node/config` does not exist, Prnsd materializes the
built-in configuration under that isolated directory. It enables transport and
the supported automatic interfaces. Interface availability is visible in
status; missing hardware does not make the configuration share your normal
Reticulum state.

## Attach, detach, and stop

`cargo prnsd` starts the managed process if needed and attaches to its log.
Ctrl-C detaches without stopping the daemon. These commands make the lifecycle
explicit:

```console
cargo prnsd --detach
cargo prnsd logs
cargo prnsd restart --debug
cargo prnsd stop
```

Repeated starts attach to the existing process rather than spawning another.
Build and daemon options are used for a stopped service; use `restart` to
replace the options of a running service. Build options go before `--`; daemon
options go after it.

## Inspect the node

`status` reports RNS network state. `interfaces` reads and edits the same
configuration the daemon plans at startup:

```console
cargo prnsd status --config target/quickstart-node
cargo prnsd interfaces --config target/quickstart-node list
cargo prnsd interfaces --config target/quickstart-node validate
```

The other prefixless utility roles are `path`, `probe`, `id`, `cp`, and `x`.
They are documented in [Prnsd utilities](../docs/prnsd-utilities.md).

## Configure a node

Prnsd reads the stock Reticulum ConfigObj format and uses the standard
configuration locations. Pass `--config DIR` to select `DIR/config`. The
interactive editor preserves comments and unrelated settings:

```console
cargo prnsd interfaces --config target/quickstart-node
```

Scripted mutations support validation, dry-run diffs, safe repair, and explicit
live apply. Read [Prnsd configuration](../docs/prnsd-config.md) before operating
real interfaces or remote management.

Prnsd also serves safe recursive `.mu` files from `nnpages/pages/` in that
configuration directory and downloads from `nnpages/files/`. File edits are
read live, and path additions or removals reconcile every five minutes. Apply
them immediately with:

```console
cargo prnsd nnpages refresh --config target/quickstart-node
```

NNPages commands are friendlier with a managed daemon: when `--config` is
omitted they target the active `cargo prnsd` session, then fall back to the
normal platform Reticulum directory when no managed daemon is running. Seed the
complete editable layout or change its announced name with:

```console
cargo prnsd nnpages seed
cargo prnsd nnpages rename "My Node"
```

Foreground and container daemons using a nondefault directory still need an
explicit `--config`.

## Build and API contract

```console
cargo prnsd build
cargo prnsd -- --help
```

The first command produces the locked release-profile daemon artifact. The
second prints the daemon's direct options without starting a managed session.
For lifecycle logs and structured observability, see
[Observability](../docs/observability.md).

For signed native releases, the production container, one-time cloud bootstrap,
operator-owned NomadNet pages, Railway deployment, backups, upgrades, and
verification, see
[Deploy prnsd](../docs/deploy-prnsd.md).

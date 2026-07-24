# Getting Started

This guide gets a fresh clone to five useful outcomes: run a node, inspect it,
run a Rust consumer, test a change, and measure performance.

## Check the host

From the repository root:

```console
./tools/prns doctor getting-started
```

The doctor reports missing commands and important version mismatches. It prints
platform-specific setup guidance but never installs or changes host software.
Use `tools\prns.cmd` instead of `./tools/prns` on Windows.

## Run and inspect a node

Follow the [Prnsd guide](../prnsd/README.md) for an isolated configuration,
managed start, interface inspection, log attachment, and clean stop.

## Run a Rust consumer

```console
./tools/prns doctor rust
cargo tools guide rust
```

The example creates two real nodes with fresh identities. Node A listens on an
OS-selected localhost TCP port, Node B connects, and Node B exits only after it
observes Node A's Reticulum announce through its TCP client. See the
[Personal RNS guide](../personal-rns/README.md) for the complete invocation and
the API anatomy.

The default does not activate discovery hardware. To additionally attach Node
B's Wi-Fi, USB, and Bluetooth auto interfaces:

```console
cargo tools guide rust -- --with-auto
```

That opt-in may multicast, advertise or request Bluetooth permission, and open
compatible USB devices. Missing adapters or peers do not make the example fail;
the localhost TCP path remains the executable contract.

## Build an embedded node

The [embedded guide](embedded.md) starts with a non-flashing XIAO ESP32-C6
firmware build, then traces the same `PrnsNodeRecipe` through hardware bring-up,
fixed storage, interfaces, identities, and static NomadNet routes. It uses a
shipped board application so the example is honest about the obligations a
bare-metal host must supply.

## Test a change

```console
cargo test --locked
```

That is the normal core path. The [testing guide](testing.md) explains the
workspace, integration, platform, and longer PR lanes.

## Measure performance

```console
./tools/prns doctor benchmarks
cargo benchmark --smoke
```

The smoke run validates the benchmark machinery without making a publishable
performance claim. Continue in the [benchmark guide](../benchmarks/README.md).

## Run these guides locally

```console
./tools/prns doctor docs
cargo run -p docs
```

The hosted/default site build renders the canonical Markdown files from this
clone. The compact embedded-device site intentionally does not include them.

## Find deeper operations

`./tools/prns` is bootstrap-safe; `cargo tools` is the convenient post-Rust
alias into the same registry:

```console
./tools/prns list
cargo tools explain guide.rust
python3 validation/run.py list --platform current
```

Read [repository tools](../tools/README.md) for the control plane and
[validation](validation.md) for the evidence model.

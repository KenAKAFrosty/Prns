# Getting Started

This guide is a progressive path through the repository: understand what Prns
owns, obtain one real result, inspect the smallest consumer, then choose the
capability that matches your application.

## Understand Prns

Prns is one Reticulum engine with bounded APIs for firmware, browsers, native
applications, and daemons. Interfaces and host adapters depend inward on that
engine, while compatibility, performance, and release claims are backed by
checked-in tests and reproducible evidence.

## Check the host

From the repository root:

```console
./tools/prns doctor getting-started
```

The doctor reports missing commands and important version mismatches. It prints
platform-specific setup guidance but never installs or changes host software.
Use `tools\prns.cmd` instead of `./tools/prns` on Windows.

## Obtain one result

```console
./tools/prns doctor rust
cargo tools guide rust
```

The example creates two real nodes with fresh identities. Node A listens on an
OS-selected localhost TCP port, Node B connects, and Node B exits only after it
observes Node A's Reticulum announce through its TCP client. See the
[Personal RNS guide](../personal-rns/README.md) for the complete invocation and
the API anatomy. It does not activate discovery radios or require an existing
Reticulum network.

## Inspect the example

Read
[`personal-rns/examples/node_basics.rs`](../personal-rns/examples/node_basics.rs).
It is intentionally small enough to show the complete ownership shape: node
recipes own state and events, handles issue commands and attach interfaces, and
the process exits through a bounded success condition.

The default does not activate discovery hardware. To additionally attach Node
B's Wi-Fi, USB, and Bluetooth auto interfaces:

```console
cargo tools guide rust -- --with-auto
```

That opt-in may multicast, advertise or request Bluetooth permission, and open
compatible USB devices. Missing adapters or peers do not make the example fail;
the localhost TCP path remains the executable contract.

## Choose a capability

| Goal | Continue here |
| --- | --- |
| Run and inspect a managed node | [Prnsd guide](../prnsd/README.md) |
| Send requests, resources, or change interfaces | [Example catalog](examples.md) |
| Own a node inside a long-lived application | [Application integration](application-integration.md) |
| Build a Rust consumer | [Personal RNS guide](../personal-rns/README.md) |
| Build a browser or TypeScript consumer | [JavaScript package guide](https://github.com/KenAKAFrosty/Prns/blob/main/prns-js/README.md) |
| Build against a native SDK | [Native binding guides](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/README.md) |
| Build a board-backed node | [Embedded guide](embedded.md) |
| Develop or physically qualify Hopspot | [Personal Hopspot guide](../personal-hopspot/README.md) |
| Validate a change | [Testing guide](testing.md) |
| Measure performance | [Benchmark guide](../benchmarks/README.md) |

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

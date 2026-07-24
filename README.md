# Prns

<p align="center">
  <a href="https://prns.dev" target="_blank">
  <img src="docs/website/public/assets/og.png" alt="Prns: a high-performance port of Reticulum (RNS). Runs on any device." width="800" />
  </a>
</p>

[![CI](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml/badge.svg)](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.90-orange.svg)](#minimum-supported-rust-version)
[![no_std](https://img.shields.io/badge/no__std-core-success.svg)](#embedded-and-no_std)

Prns is a Rust implementation of Reticulum for applications, daemons, browsers,
phones, and embedded devices. This clone contains the code, essential guides,
tests, benchmarks, and a locally runnable documentation site.

## Prerequisites

The common paths need Git, Rust 1.90 or newer, and Python 3.11 or newer. Check
your machine without installing anything:

```console
./tools/prns doctor getting-started
```

On Windows, use `tools\prns.cmd`. First-time dependency downloads may require
network access, but the instructions and source material are all in this clone.

## What do you want to do?

| Outcome | Start here |
| --- | --- |
| Learn the repository | [Getting started](docs/getting-started.md) |
| Run and inspect a node | [Prnsd guide](prnsd/README.md) |
| Build a Rust application | [Personal RNS guide](personal-rns/README.md) |
| Build an embedded node | [Embedded Prns guide](docs/embedded.md) |
| Test a change | [Testing guide](docs/testing.md) |
| Measure performance | [Benchmark guide](benchmarks/README.md) |
| Build the local website | [Website README](docs/website/README.md) |
| Discover repository operations | [Repository tools](tools/README.md) |

## First commands

Run the safe two-node Rust contract. It creates fresh identities, binds only to
localhost, observes a real Reticulum announce, and exits:

```console
cargo tools guide rust
```

Run the normal core test path:

```console
cargo test --locked
```

Serve the documentation site from the clone:

```console
cargo run -p docs
```

Enable the repository hooks once per clone:

```console
git config core.hooksPath .githooks
```

## Embedded and `no_std`

`prns-core` supports `no_std` builds from an alloc-free, fixed-capacity profile
through `no_std + alloc`. The Embassy runtime and interface implementations
carry the same engine onto ESP32 and nRF52840 firmware targets. Follow the
[board-backed embedded guide](docs/embedded.md) to build a real XIAO ESP32-C6
node and trace the shared node recipe into its hardware obligations.

## Minimum supported Rust version

The workspace's declared and CI-tested MSRV is Rust **1.90**. Development builds
use the stable channel configured in [rust-toolchain.toml](rust-toolchain.toml).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and the
[testing guide](docs/testing.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.

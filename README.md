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

## Getting started

Visit [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs).

The clone guides you on its own. With a stable Rust toolchain and Python 3.11
or newer:

- `cargo test` builds and tests the core workspace.
- `./tools/prns list` discovers every supported build, device, release, and
  repository operation; `./tools/prns explain TASK_ID` details one, and
  `./tools/prns doctor` reports what your host is missing.
- `python3 validation/run.py list` (`python` on Windows) discovers every test,
  proof, and interoperability suite.
- `git config core.hooksPath .githooks` enables the repository hooks, once per
  clone.

Building a Node.js, Electron, or Tauri application? The full node is on npm as
[`personal-rns`](prns-napi/README.md) — the complete engine as a native addon,
with no daemon required.

## Embedded and `no_std`

`prns-core` supports `no_std` builds from an alloc-free, fixed-capacity profile
through `no_std + alloc`. The Embassy runtime and interface implementations
carry the same engine onto ESP32 and nRF52840 firmware targets.

## Minimum supported Rust version

The workspace's declared and CI-tested MSRV is Rust **1.90**. Development builds
use the stable channel configured in [rust-toolchain.toml](rust-toolchain.toml).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

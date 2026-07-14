# Prns


<p align="center">
  <a href="https://prns.dev" target="_blank">
  <img src="docs/website/public/assets/og.png" alt="Prns: a high-performance port of Reticulum (RNS). Runs on any device." width="800" />
  </a>
</p>

<!--
Badges are deliberately limited to state that is real and verifiable today.
No crates.io / docs.rs badges: every crate is `publish = false`, so those would 404.
No coverage badge: CI does not upload coverage yet, so it would sit broken.
The CI badge reads "no status" to anonymous viewers until the repo is public.
-->
[![CI](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml/badge.svg)](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.90-orange.svg)](#minimum-supported-rust-version)
[![no_std](https://img.shields.io/badge/no__std-core-success.svg)](#embedded-and-no_std)

## Getting Started
Visit the docs website at either https://prns.dev or https://reticulum.rs, or run the docs site locally from source. 


## Minimum supported Rust version

The workspace builds on Rust **1.90** and newer, and tracks `stable` (pinned in [rust-toolchain.toml](rust-toolchain.toml)). 1.90 is the declared workspace MSRV; the `no_std` core itself compiles on older toolchains.

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

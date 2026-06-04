# Personal Reticulum Suite

A ground-up Rust implementation of the Reticulum Network Stack and the LXMF messaging layer: one pure, `no_std` engine that each platform hosts behind a thin shim.

<!--
Badges are deliberately limited to state that is real and verifiable today.
No crates.io / docs.rs badges: every crate is `publish = false`, so those would 404.
No coverage badge: CI does not upload coverage yet, so it would sit broken.
The CI badge reads "no status" to anonymous viewers until the repo is public.
-->
[![CI](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml/badge.svg)](https://github.com/KenAKAFrosty/Prns/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-orange.svg)](#minimum-supported-rust-version)
[![no_std](https://img.shields.io/badge/no__std-core-success.svg)](#embedded-and-no_std)


## Overview

<!-- TODO: 2 to 3 sentences framing what Reticulum is and why a from-scratch Rust port exists. Keep it grounded and link the upstream spec. -->

The core engine is `no_std` and allocation-free, so the same wire contract runs on a Linux daemon and on a microcontroller. Platform specifics live in thin host layers and language bindings around the engine, never inside it.

## Workspace layout

| Crate | Role |
| --- | --- |
| [`personal-rns`](personal-rns) | Pure Reticulum engine and wire contract (`no_std`, alloc-free core). |
| [`personal-rnsd`](personal-rnsd) | Thin daemon host for the engine. |
| [`personal-lxmf`](personal-lxmf) | LXMF application layer above the engine. |
| [`personal-rns-ffi`](personal-rns-ffi) | uniffi bindings: Kotlin, Swift, and Python from one UDL. |
| [`personal-rns-napi`](personal-rns-napi) | Node-API bindings, published as the `@personal/rns` TypeScript package. |
| [`personal-rns-capi`](personal-rns-capi) | C ABI for C, C++, Go, Zig, and other native consumers. |

Also in the repository: [`rvt`](rvt) (Reticulum Visual Toolkit, multi-node simulation and dev tooling), [`personal-hopspot`](personal-hopspot) (embedded status-screen host), [`fuzz`](fuzz) (fuzz targets), and the device host workspaces under [`hosts/`](hosts).

## Status

This is pre-release software (`0.1.0`). The wire contract and public API may still change.

<!-- TODO: note current interop coverage against upstream Reticulum, and what "production grade" means for this milestone. -->

## Getting started

<!-- TODO: prerequisites (stable Rust is selected automatically via rust-toolchain.toml), then the canonical build and test commands, the personal-rnsd entrypoint, and a minimal send-a-message walkthrough once the API settles. Keep examples in sync with each crate's examples directory. -->

## Usage

<!-- TODO: minimal end-to-end example. Start with the daemon, then one binding (likely the C or Node API), mirrored by a runnable example under the relevant crate. -->

## Embedded and `no_std`

The `personal-rns` core builds `no_std` and allocation-free, and CI cross-compiles it to `riscv32imac-unknown-none-elf` (ESP32-C6 class) on every push. Entropy is supplied by the host as data; the core never owns an RNG.

<!-- TODO: list the feature flags (embassy-seam / embassy-contract / embassy-host) and which board class each targets. -->

## Documentation

- [docs/validation.md](docs/validation.md): the fuzzing, property-test, and mutation-test lanes.

<!-- TODO: link rendered API docs once a docs target is published. -->

## Validation and testing

CI runs formatting, clippy with warnings denied, the full test suite, the `no_std` embedded cross-build, and `cargo-deny` for license and advisory policy. The extra proof lanes are described in [docs/validation.md](docs/validation.md).

## Minimum supported Rust version

The workspace builds on Rust **1.88** and newer, and tracks `stable` (pinned in [rust-toolchain.toml](rust-toolchain.toml)). The floor is set by the `napi` dependencies of the `personal-rns-napi` binding; the `no_std` core itself compiles on older toolchains. The `msrv` CI job pins 1.88 so the declared version cannot silently drift.

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

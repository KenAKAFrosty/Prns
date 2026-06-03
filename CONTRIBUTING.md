# Contributing

Thanks for your interest in Personal Reticulum Suite. This is pre-release software and the wire contract is still settling, so please open an issue to discuss any non-trivial change before you start.

## Ground rules

The guiding principle for this codebase is the build directive: port the contract, not the implementation. Read [docs/build-ethos.md](docs/build-ethos.md) before proposing engine changes. In short:

- The `personal-rns` core stays `no_std` and allocation-free. Platform code belongs in a host layer or a binding crate, never in the core.
- Entropy and I/O are supplied by the host as data. The core does not own an RNG, a clock, or a socket.

## Development setup

The toolchain is pinned in [rust-toolchain.toml](rust-toolchain.toml) and selected automatically.

The CI gates are the source of truth (see [.github/workflows/ci.yml](.github/workflows/ci.yml)): formatting, clippy with warnings denied, tests, the `no_std` embedded cross-build, and `cargo-deny`. Run the fast checks locally before opening a pull request:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Commit and pull request conventions

<!-- TODO: state branch naming, commit message style, and review expectations. -->

## License of contributions

By contributing, you agree that your contributions will be dual licensed under the MIT and Apache-2.0 licenses, matching the project's [license](README.md#license).

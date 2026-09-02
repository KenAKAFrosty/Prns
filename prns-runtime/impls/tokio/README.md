# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

## Process model

The runtime supports ordinary process creation that starts a new program, including
`std::process::Command`. Continuing to use an inherited runtime after a raw Unix `fork` is not
supported. A forked child must execute a fresh program image before using Prns; continuing within
the inherited process could reuse cryptographic random-generator state from its parent.

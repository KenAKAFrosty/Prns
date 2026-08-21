# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

`prns-macros` contains dependency-free declarative macros shared by the project.

`iterable_enum!` declares a unit-only enum and derives its complete `ALL` array from the same
variant list, so adding a variant cannot leave iteration or dense array sizing out of sync. By
default, `ALL` inherits the enum's visibility. A trailing `const ALL;` declaration is only needed
to override that visibility or attach attributes such as `#[cfg(test)]`.

This crate is `no_std` and contains no procedural macros or runtime dependencies.

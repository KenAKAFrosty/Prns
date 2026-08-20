# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. Quick overviews, the complete feature guide, API documentation, examples, and the cross-language SDK overview are available at [prns.dev](https://prns.dev) or [reticulum.rs](https://reticulum.rs), and in the [source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual MIT/Apache-2.0 license.

# Hopspot

`hopspot` is an alternate public package name for `personal-rns`. It is a
transparent facade, not a fork or a separate implementation. Both names expose
the same types, functions, modules, runtime behavior, and release version.

Use whichever name best fits the application:

```console
cargo add hopspot
npm install hopspot
```

Rust consumers receive the complete `personal-rns` public API through a direct
re-export. Every `personal-rns` Cargo feature has a same-named `hopspot` feature
that forwards to it.

```rust
use hopspot::{DestinationHash, PrnsNodeApi};
```

JavaScript and TypeScript consumers receive the same root, `native`, `browser`,
and `casework` exports as `personal-rns`.

```javascript
import { Prns, Tag } from "hopspot";
```

Versions are published in lockstep and each `hopspot` release depends on the
exact matching `personal-rns` release. Documentation and issue tracking remain
centralized in the [Prns source repository](https://github.com/KenAKAFrosty/Prns).

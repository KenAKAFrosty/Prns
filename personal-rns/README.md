# Personal RNS (Prns)

This crate is one package in the Personal RNS public Rust graph. The quick overviews, complete
feature guide, API documentation, examples, and cross-language SDK overview are
accessible via [reticulum.rs](https://reticulum.rs) and in the
[source repository](https://github.com/KenAKAFrosty/Prns).

All public packages use the same engine, release version, and dual
MIT/Apache-2.0 license.

## Rust SDK

`personal-rns` brings the pure Reticulum engine, the high-level runtime, and
every interface family behind one crate and one prelude. The core is `no_std`
and does not require `alloc`; applications opt into the runtime and interfaces
their target can actually provide.

## Install

For a Tokio application using TCP:

```console
cargo add personal-rns --features tokio-host,tcp
```

For the exact source candidate before its announced release:

```console
cargo add personal-rns --git https://github.com/KenAKAFrosty/Prns --features tokio-host,tcp
```

The default feature set is intentionally thin. Choose
`tokio-host` for native async applications or `embassy-host` for embedded async
applications, then enable the interface families the application uses.

## Run the first complete journey

From a source checkout:

```console
./tools/prns doctor getting-started
cargo tools guide rust-basics
```

That example creates two nodes, binds an isolated local TCP connection,
announces a real destination, verifies the signed announce on the second node,
and exits on a bounded success condition. Its complete source is
[`examples/node_basics.rs`](examples/node_basics.rs).

The [Getting Started guide](../docs/getting-started.md) explains the recipe and
then removes the explicit address through automatic LAN discovery. The
[example catalog](../docs/examples.md#rust) continues through transport,
request and response, typed application state, resource transfer, live
interface changes, and persistence.

## Runtime shape

A `PrnsNodeRecipe` declares the node's destinations, storage, event handler,
interfaces, persistence, and application state. `PrnsNode::new` produces the
running node, while its handle issues commands and changes interfaces without
moving protocol ownership out of the runtime.

The public prelude is the normal application entrance:

```rust
use personal_rns::prelude::*;
```

Feature names keep platform costs explicit. Common choices include:

| Need | Features |
| --- | --- |
| Native async host | `tokio-host` |
| Embedded async host | `embassy-host` |
| TCP | `tcp` |
| Automatic LAN discovery | `wifi-auto` |
| Serial, KISS, AX.25 KISS, or RNode | `serial`, `kiss`, `ax25`, `rnode` |
| WebSocket or I2P | `websocket`, `i2p` |
| USB or Bluetooth discovery | `usb`, `bluetooth-auto` |
| LoRa or ESP-NOW on embedded targets | `lora`, `esp-now` |

See the [complete SDK guide](../docs/sdks.md) for release readiness and the
[embedded guide](../docs/embedded.md) for allocation-free and hardware-backed
targets.

Personal RNS is dual licensed under MIT or Apache-2.0.

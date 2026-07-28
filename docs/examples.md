# Example Catalog

Every example here is a small, complete program. Each one exercises one concept from [More Key Concepts](more-concepts.md), so the fastest way to learn the API is to run an example, then open its file and read how it did what you just watched.

## Rust

These seven Rust examples form a ladder (We suggest you run them strictly in order as listed below). 

Every one runs from a fresh clone and ends on a bounded success condition, so you always know whether what you just ran actually worked.

| Example | What you'll see |
| --- | --- |
| [`node_basics.rs`](../personal-rns/examples/node_basics.rs) | Two nodes built from recipes; one announces, the other hears the signed [Announce](more-concepts.md#the-essentials) and verifies it over localhost TCP. This is the quickstart from [Getting Started](getting-started.md): `cargo tools guide rust-basics`. |
| [`auto_discovery.rs`](../personal-rns/examples/auto_discovery.rs) | No addresses anywhere in the code. Both nodes turn on Wi-Fi/LAN auto-discovery and find each other anyway, then keep listening for announces from other machines on your network: `cargo tools guide rust-auto-discovery`. |
| [`transport_node.rs`](../personal-rns/examples/transport_node.rs) | Three nodes, two links: a [Transport node](more-concepts.md#the-essentials) relays an announce between two nodes that never touch each other: `cargo tools guide rust-transport-node`. |
| [`bounded_request.rs`](../personal-rns/examples/bounded_request.rs) | Register a [request endpoint](more-concepts.md#request-and-response) and answer a peer's request over a link, like an API route with no server between you: `cargo tools guide rust-bounded-request`. |
| [`app_state.rs`](../personal-rns/examples/app_state.rs) | Your node's typed app state serving requests: an endpoint reads and updates a shared `StatusBoard` on every hit, the stand-in for your database or cache: `cargo tools guide rust-app-state`. |
| [`resource_transfer.rs`](../personal-rns/examples/resource_transfer.rs) | Send 64 KiB over a link as a [Resource](more-concepts.md#resource) and get verified settlement (proof the data arrived intact): `cargo tools guide rust-resource-transfer`. |
| [`dynamic_interface.rs`](../personal-rns/examples/dynamic_interface.rs) | Attach a new [Interface](more-concepts.md#the-essentials) to a running node, observe it live, then tear it down and observe its removal: `cargo tools guide rust-dynamic-interface`. |

If you're familiar with Rust and want to de-sugar the command, you can run the focused examples directly:

```console
cargo run --locked -p personal-rns --example bounded_request --features tokio-host,tcp
cargo run --locked -p personal-rns --example resource_transfer --features tokio-host,tcp
cargo run --locked -p personal-rns --example dynamic_interface --features tokio-host,tcp
```

## TypeScript

| Example | What you'll see |
| --- | --- |
| [`native-lifecycle.ts`](../prns-js/examples/native-lifecycle.ts) | Create a native node, claim its event stream, attach and detach an interface, and stop cleanly, with every event and command case handled exhaustively. |
| [`browser-resource.ts`](../prns-js/examples/browser-resource.ts) | Create a node in the browser and send a `Blob` as bounded resource segments, handling failure right where it can happen. |
| [Browser transport playground](/prns-wasm/examples/browser-playground/README.md) | A live WebAssembly node in your browser, with WebUSB and Wi-Fi kept behind explicit permission clicks. |

Typecheck both source examples:

```console
npm --prefix prns-js run check
```


## Native SDKs

Every native SDK follows the same recipe: create a host with explicit capabilities, claim the application event stream once, execute typed commands, and handle each settlement as plain success or failure data. The guides below show that recipe in each language's own idiom.

| SDK | Authoritative recipe |
| --- | --- |
| Python | [Create, claim, and consume frozen event variants](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/python/README.md) |
| .NET | [Create, claim, consume, and settle generated records](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/dotnet/README.md) |
| Go | [Create, execute, wait with context, and switch on settlement](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/go/README.md) |
| Swift | [Claim an `AsyncSequence`, execute, and switch on settlement](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/swift/README.md) |
| Kotlin, Java, and Android | [Create, execute, await, and close deterministic owners](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/jvm/README.md) |
| Julia | [Create, claim, execute, and wait through multiple dispatch](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/julia/README.md) |
| C and C++ | [Create opaque owners, pull one stream, and release each handle](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/abi/c/README.md) |

The native package graph and the validation suites that exercise these adapters live in the [host-contract guide](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/README.md).

## Community examples

Examples and projects built outside this repository belong here too, and this section is meant to grow. Nothing is listed yet; yours could be the first.

If you build something with Prns - a demo, a tool, a full application - we'd love to see it. [Open a issue](https://github.com/KenAKAFrosty/Prns/issues) telling us about it, or send a pull request adding your link right here.
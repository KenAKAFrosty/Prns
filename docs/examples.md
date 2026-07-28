# Example Catalog

Every example below states what its label means:

- **Runnable** executes from this clone and has a bounded success condition.
- **Compile-checked** is part of a language compiler or typechecker gate.
- **Browser-hosted** is built into the local documentation playground and
  requires an explicit browser permission action for hardware.
- **Illustrative** communicates architecture or ownership but is not presented
  as a copy-and-run program.

No example implies that its package has already been published to a registry.
Use source-tree commands unless an authoritative package guide explicitly
documents an available release.

## Rust

| Example | Label | Result |
| --- | --- | --- |
| [`node_basics.rs`](../personal-rns/examples/node_basics.rs) | Runnable | Two localhost nodes exchange a real announce; `cargo tools guide rust` is the Rust quickstart. |
| [`bounded_request.rs`](../personal-rns/examples/bounded_request.rs) | Runnable | A registered request endpoint answers a link request before a ten-second deadline. |
| [`resource_transfer.rs`](../personal-rns/examples/resource_transfer.rs) | Runnable | A bounded 64 KiB resource transfer settles successfully against an accepting localhost peer. |
| [`dynamic_interface.rs`](../personal-rns/examples/dynamic_interface.rs) | Runnable | A running node attaches a TCP supervisor, observes it, tears it down, and observes its removal. |

Run the focused examples from the repository root:

```console
cargo run --locked -p personal-rns --example bounded_request --features tokio-host,tcp
cargo run --locked -p personal-rns --example resource_transfer --features tokio-host,tcp
cargo run --locked -p personal-rns --example dynamic_interface --features tokio-host,tcp
```

## TypeScript

| Example | Label | Result |
| --- | --- | --- |
| [`native-lifecycle.ts`](../prns-js/examples/native-lifecycle.ts) | Compile-checked | Creates a native node, claims the single-owner event stream, exhaustively handles event and command cases with `casework`, attaches and detaches an interface, and stops. |
| [`browser-resource.ts`](../prns-js/examples/browser-resource.ts) | Compile-checked | Creates a cooperative browser node, handles the early create outcome, sends a `Blob` as bounded resource segments, and exhaustively handles failure. |
| [Browser transport playground](https://github.com/KenAKAFrosty/Prns/blob/main/prns-wasm/examples/browser-playground/README.md) | Browser-hosted | Runs a WebAssembly node and keeps WebUSB and Wi-Fi permission boundaries behind explicit user actions. |

Typecheck both source examples:

```console
npm --prefix prns-js run check
```

Both use tagged outcomes from `casework`; expected failures are handled near the
operation that produced them instead of becoming late exceptions.

## Native SDKs

All native adapters implement the same recipe: create a host with explicit
capabilities, claim an application event stream once, execute a typed command,
then handle the command settlement as success or failure data. Their
authoritative guides use each language's ownership and cancellation idioms.

| SDK | Label | Authoritative recipe |
| --- | --- | --- |
| Python | Compile-checked | [Create, claim, and consume frozen event variants](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/python/README.md) |
| .NET | Compile-checked | [Create, claim, consume, and settle generated records](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/dotnet/README.md) |
| Go | Compile-checked | [Create, execute, wait with context, and switch on settlement](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/go/README.md) |
| Swift | Compile-checked | [Claim an `AsyncSequence`, execute, and switch on settlement](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/swift/README.md) |
| Kotlin, Java, and Android | Compile-checked | [Create, execute, await, and close deterministic owners](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/jvm/README.md) |
| Julia | Compile-checked | [Create, claim, execute, and wait through multiple dispatch](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/bindings/julia/README.md) |
| C and C++ | Compile-checked | [Create opaque owners, pull one stream, and release each handle](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/abi/c/README.md) |

The native package graph and exact validation suite IDs live in the
[host-contract guide](https://github.com/KenAKAFrosty/Prns/blob/main/prns-host/README.md). Building or validating these
adapters does not publish them.

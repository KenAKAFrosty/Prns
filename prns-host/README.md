# Prns host contract

`prns-host` is the language-neutral behavioral center of every hosted Prns SDK. It defines the release ABI, semantic configuration, capabilities, lifecycle, bounded command and event lanes, diagnostic gaps, resource streams, and single-owner consumer claims. The canonical machine-readable source is `schema/host-contract-v1.json`; Rust, TypeScript, C, .NET, and portable conformance vectors are generated from it.

The implementation is deliberately split into ownership layers:

| Layer | Owns |
| --- | --- |
| `prns-host/core` | Stable vocabulary, limits, admission policy, lifecycle, and backend-independent event semantics |
| `prns-host/impls/cooperative` | Explicit time and entropy for browsers, embedded executors, and caller-driven hosts |
| `prns-host/impls/tokio` | Blocking and asynchronous native-host scheduling |
| `prns-host/abi/c` | Stable opaque-handle native capsule and pull-based event/resource ownership |
| `prns-host/bindings/dotnet` | `SafeHandle`, bounded `Channel<T>`, `IAsyncEnumerable<T>`, sealed records, and exhaustive `Match` |

A language binding translates this contract; it does not invent another runtime model. Expected outcomes remain explicit unions, application data remains lossless until a declared terminal failure, diagnostics may drop newest with an exact gap count, and every application, diagnostic, or resource stream has one consumer.

## Binding shape

The same concepts have idiomatic presentations:

| Contract concept | TypeScript | .NET | Python | Swift/Kotlin |
| --- | --- | --- | --- | --- |
| Tagged outcome | casework union | sealed record hierarchy | frozen variant classes | enum/sealed hierarchy |
| Application stream | `AsyncIterableIterator<T>` | `IAsyncEnumerable<T>` | async iterator | `AsyncSequence`/`Flow` |
| Stream claim | `Claimed \| AlreadyClaimed` | `Claimed<T> \| AlreadyClaimed` | explicit result variant | explicit enum/sealed result |
| Resource body | claimed chunk stream | claimed `IAsyncEnumerable<ReadOnlyMemory<byte>>` | claimed async byte chunks | claimed async byte chunks |
| Lifecycle | exhaustive tagged state | exhaustive sealed state | exhaustive variant state | exhaustive enum/sealed state |

Bindings must verify `HOST_CONTRACT_ABI` and product version before creating a node. Backend-specific features appear through capabilities or explicit sub-interfaces, never optional methods that fail later.

## Contract workflow

Change the schema first. Generated files are never edited directly.

```sh
./tools/prns run repo.host-contract.generate
./tools/prns run repo.host-contract.check
cargo test --manifest-path prns-host/abi/c/Cargo.toml
npm --prefix prns-js test
```

The generator rejects duplicate names or discriminants, unknown field types, disagreement between event unions and event-kind enums, and product-version disagreement across Rust, npm, C, and .NET packages. CI then compiles the generated C header, exercises the C capsule, type-checks TypeScript, and compiles the .NET exhaustive-match smoke.

## Expansion order

.NET is the first native binding over the capsule: opaque handles feed a bounded `Channel<T>`, expose `IAsyncEnumerable<T>`, and use generated sealed records plus exhaustive `Match` helpers without changing the host contract.

Python follows through the same native capsule with async generators and frozen variants. Swift and Kotlin then reuse the capsule and map the same cases to `AsyncSequence` and `Flow`. Go is mechanically straightforward after the capsule exists, although cancellation and goroutine ownership need a deliberately narrower wrapper than the native Go channel defaults.

Every new binding must pass the same conformance vectors for ABI mismatch, queue pressure, exact diagnostic gaps, second-consumer rejection, lifecycle terminality, and resource ownership before convenience helpers are added.

# Prns host contract

`prns-host` is the language-neutral behavioral center of every hosted Prns SDK. It defines the release ABI, semantic configuration, capabilities, lifecycle, bounded command and event lanes, diagnostic gaps, resource streams, and single-owner consumer claims.

The implementation is deliberately split into three layers:

| Layer | Owns |
| --- | --- |
| `prns-host/core` | Stable vocabulary, limits, admission policy, lifecycle, and backend-independent event semantics |
| `prns-host/impls/cooperative` | Explicit time and entropy for browsers, embedded executors, and caller-driven hosts |
| `prns-host/impls/tokio` | Blocking and asynchronous native-host scheduling |

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

## Expansion order

.NET is the next native binding: an opaque-handle C ABI can feed bounded callbacks into `Channel<T>`, expose `IAsyncEnumerable<T>`, and generate sealed records plus exhaustive `Match` helpers without changing the host contract.

Python follows through the same native capsule with async generators and frozen variants. Swift and Kotlin then reuse the capsule and map the same cases to `AsyncSequence` and `Flow`. Go is mechanically straightforward after the capsule exists, although cancellation and goroutine ownership need a deliberately narrower wrapper than the native Go channel defaults.

Every new binding must pass the same conformance vectors for ABI mismatch, queue pressure, exact diagnostic gaps, second-consumer rejection, lifecycle terminality, and resource ownership before convenience helpers are added.

# Personal RNS host contract

`prns-host` is the language-neutral center of every hosted Personal RNS SDK.
The canonical schema in `schema/host-contract-v1.json` owns stable
discriminants, fixed byte widths, semantic configuration, commands, outcomes,
application events, diagnostics, and resource streams. Deterministic generation
projects that vocabulary into Rust, TypeScript, C, C#, Python, Go, Swift,
Kotlin, and Julia.

The native execution path has one real implementation:

```text
language package
    -> idiomatic ownership and async adapter
        -> versioned C ABI with opaque handles
            -> native Rust host
                -> Personal RNS engine
```

No language binding reimplements routing, Reticulum semantics, queue policy, or
event meaning. The C capsule is the binary lighthouse: Rust enums, allocators,
futures, and unwinding never cross it; owned opaque handles, size-prefixed
structures, fixed-width discriminants, borrowed views, and explicit status
values do.

## Ownership layers

| Layer | Owns |
| --- | --- |
| `core` | Stable vocabulary, limits, lifecycle, admission policy, and backend-independent event semantics |
| `impls/native` | The real threaded/Tokio host, semantic configuration, bounded command submission, and interruptible waits |
| `impls/cooperative` | Explicit time and entropy for browsers, embedded executors, and caller-driven hosts |
| `impls/tokio` | Blocking and asynchronous native-host scheduling |
| `abi/c` | Stable opaque handles, pull-based events/resources, panic containment, and ABI/version gates |
| `bindings/*` | Idiomatic types, cancellation, deterministic ownership, package metadata, and no new protocol semantics |

## Ecosystem shape

| Ecosystem | Typed cases | Async/event surface | Distribution |
| --- | --- | --- | --- |
| TypeScript, Node, Bun, browser | `casework` unions and exhaustive `match()` | `AsyncIterableIterator<T>` | one `personal-rns` npm package with native and browser exports |
| .NET | sealed records and exhaustive `Match` helpers | `IAsyncEnumerable<T>` | `PersonalRns` NuGet with runtime-specific native assets |
| Python | frozen variant classes | async iterators | platform wheels containing the native capsule |
| Go | generated closed interfaces and concrete cases | context-aware pull streams | Go module plus native release archive |
| Swift | generated enums | `AsyncSequence` | Swift Package plus native release archive/pkg-config |
| Kotlin, Java, Android | generated sealed interfaces | one-shot Kotlin `Flow` | Maven JAR; Android uses the same API with JNA AAR and per-ABI native assets |
| Julia | generated abstract types and concrete structs | Julia iteration/tasks | Julia package plus native release archive |
| C and C++ | fixed enums, structs, and opaque handles | interruptible blocking pull | content-addressed native artifact with header, static/dynamic libraries, and pkg-config |

Commands settle as an explicit success or failure case. Cancellation does not
abandon a blocked foreign thread: each adapter interrupts the native wait and
joins its ownership boundary before release. Application and diagnostic lanes
are claimed once. Application data remains lossless inside declared bounds;
diagnostics may drop newest and report an exact accumulated gap. Resource bodies
have one consumer and retain their own native handle after the parent event is
released.

## Contract workflow

Change the schema first. Generated files are never edited directly.

```sh
./tools/prns run repo.host-contract.generate
./tools/prns run repo.host-contract.check
python3 tools/release/check-host-sdk-versions.py
python3 validation/run.py verify
```

The generator rejects duplicate names or discriminants, unknown field types,
event-union disagreement, and stale language projections. Registered native
smokes then exercise real creation, ABI/product mismatch gates, stream
single-ownership, wait interruption, and command settlement across C, .NET,
Python, Go, Swift, Kotlin/JVM, and Julia.

## Release assets

`tools/release/package-host-native.py` creates a relocatable target artifact
containing the generated header, dynamic and static libraries, pkg-config
metadata, both project licenses, an exact commit, and SHA-256/size records for
every shipped file. The host SDK workflow builds GNU and musl Linux, macOS,
Windows, and Android targets. Python wheels and the NuGet package consume those
same outputs; the Maven staging repository and source-first Go, Swift, and Julia
packages remain anchored to the identical contract version.

Publication is an explicit, SHA-gated workflow-dispatch action. Building and
testing never implicitly publish.

## Adding another language

Read [`bindings/README.md`](bindings/README.md). A new adapter starts from the
generated contract and C ABI, preserves the ownership/cancellation rules, maps
closed cases to the language's strongest sum-type form, and proves the same live
smoke before convenience APIs are added.

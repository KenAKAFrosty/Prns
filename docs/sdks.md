# Prns SDKs

Every Prns SDK runs the same Reticulum engine. The language layer owns types, lifetimes, cancellation, and the event-stream shape; it does not reimplement routing or wire behavior.

The implementations are at two different stages of their user journey:

| SDK | Readiness | Current entrance |
| --- | --- | --- |
| Rust | Paved | `personal-rns` crate and the complete Rust example ladder |
| TypeScript and JavaScript | Paved | One `personal-rns` package design for Node.js, Bun, and browsers |
| Python | SDK preview | Source adapter and registered live conformance suite |
| .NET and C# | SDK preview | Source adapter and registered live conformance suite |
| Go | SDK preview | Source module and registered live conformance suite |
| Swift | SDK preview | Source package and registered live conformance suite |
| Kotlin, Java, and Android | SDK preview | Source Gradle project and registered live conformance suite |
| Julia | SDK preview | Source project and registered live conformance suite |
| C and C++ | SDK preview | Generated C ABI and registered live C and C++ conformance suite |

Here, **paved** means the API, examples, and package structure form the route we expect application developers to take. **SDK preview** does not mean a mock or an unfinished protocol port. These adapters already call the same native Rust host, project the same schema-1 contract, and exercise the same persistent two-node journey in the repository. What remains young is their ecosystem fit and public delivery: idiomatic package structure, registry publication, native artifact installation, and more feedback from experienced developers in each language.

Prns 0.3.4 was the first publicly announced prerelease. The current 0.3.7 immutable GitHub release artifacts and exact source commit are authoritative for its candidate bytes. Registry packages become authoritative only after their independent publication qualification completes.

## Rust

For an application using a published registry release:

```console
cargo add personal-rns --features tokio-host,tcp
```

To use the exact source behind the public 0.3.7 prerelease before registry qualification completes:

```console
cargo add personal-rns --git https://github.com/KenAKAFrosty/Prns --features tokio-host,tcp
```

The fastest source-checkout journey creates two real nodes, connects them over TCP, and succeeds only after one verifies the other's signed announce:

```console
./tools/prns doctor getting-started
cargo tools guide rust-basics
```

(On Windows, run the doctor as `.\tools\prns.cmd doctor getting-started`.)

Continue through the [Rust example ladder](examples.md#rust), or open the [`personal-rns` crate guide](../personal-rns/README.md) for runtime and feature selection.

## TypeScript and JavaScript

The package is designed as one install with runtime-selected exports:

```console
npm install personal-rns
```

- `personal-rns` selects native Node.js/Bun or browser WebAssembly through package exports.
- `personal-rns/native` fixes the native backend.
- `personal-rns/browser` fixes the cooperative WebAssembly backend.

The exact public prerelease can also be exercised from a source checkout:

```console
npm --prefix prns-napi ci
npm --prefix prns-wasm ci
npm --prefix prns-js ci
npm --prefix prns-js run test:native:full
npm --prefix prns-js run test:browser:full
```

Read the [TypeScript and JavaScript guide](../prns-js/README.md) for host creation, tagged outcomes, event streams, persistence, and bounded browser resource transfer.

## Native SDK previews

The native previews all sit above the generated [`prns_host.h`](../prns-host/abi/c/include/prns_host.h) contract. The adapters and native capsule are version-gated together; mixing arbitrary library and adapter versions is intentionally rejected.

On Linux, each registered suite builds the current native capsule and runs the language adapter through lifecycle, exclusive stream ownership, interface configuration, a real loopback connection, announce discovery, link establishment, request and response, bounded resource transfer, shutdown, restart, and persistence restoration.

| SDK | Run from the repository root | Intended public delivery |
| --- | --- | --- |
| Python | `python3 validation/run.py run --suite host-python-contract` | `personal-rns` platform wheels |
| .NET | `python3 validation/run.py run --suite host-dotnet-contract` | `PersonalRns` NuGet package with runtime assets |
| Go | `python3 validation/run.py run --suite host-go-contract` | Go module tag plus matching native archive |
| Swift | `python3 validation/run.py run --suite host-swift-contract` | Swift Package tag plus matching native archive |
| Kotlin, Java, Android | `python3 validation/run.py run --suite host-jvm-contract` | Maven package plus desktop and Android native assets |
| Julia | `python3 validation/run.py run --suite host-julia-contract` | Julia General package with matching native artifacts |
| C and C++ | `python3 validation/run.py run --suite host-c-contract` | Signed native archive with header, libraries, and pkg-config metadata |

These commands are evaluation and contributor paths, not substitutes for the planned public packages. Each SDK guide shows its current API shape:

- [Python](../prns-host/bindings/python/README.md)
- [.NET and C#](../prns-host/bindings/dotnet/README.md)
- [Go](../prns-host/bindings/go/README.md)
- [Swift](../prns-host/bindings/swift/README.md)
- [Kotlin, Java, and Android](../prns-host/bindings/jvm/README.md)
- [Julia](../prns-host/bindings/julia/README.md)
- [C and C++](../prns-host/abi/c/README.md)

## Help make an SDK feel native

Distribution for every implemented SDK is high-priority release work. The repository already contains package manifests, target matrices, artifact assembly, version checks, and public-package qualification workflows. The remaining decisions deserve people who know the conventions and failure modes of their ecosystems deeply.

Experienced maintainers can make an unusually valuable contribution by reviewing:

- package layout, native-library discovery, and platform selection;
- ownership, cancellation, and asynchronous stream idioms;
- naming and sum-type presentation;
- minimal complete examples and first-project ergonomics;
- registry metadata and clean-consumer installation behavior.

Please start with an issue or pull request rather than publishing a package name from a personal registry account. Release coordinates and signing custody are part of the project-wide release process. The [binding implementation guide](../prns-host/bindings/README.md) describes the invariants every adapter preserves, and the [release administration guide](../prns-host/distribution/ADMIN.md) records the intended distribution path.

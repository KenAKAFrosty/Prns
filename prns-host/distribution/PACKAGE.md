# Personal RNS

Personal RNS is a correct, robust, fast Reticulum implementation with one
language-neutral host contract and idiomatic SDKs for Rust, TypeScript,
JavaScript, Python, .NET, Go, Swift, Kotlin, Java, Julia, C, and C++.

Every hosted SDK delegates protocol behavior to the same native engine through
the versioned C ABI. Language packages own types, deterministic lifetime,
cancellation, and ecosystem-native streams; they do not reimplement routing or
wire semantics. Node, Bun, and browsers share the same generated TypeScript
contract, with the browser backend running the engine through WebAssembly.

The package version and contract ABI are checked before host creation. Commands
settle as typed success or failure values, event lanes have one explicit owner,
and resource bodies retain their own bounded stream lifetime.

- Documentation: [reticulum.rs](https://reticulum.rs)
- Project: [prns.dev](https://prns.dev)
- Source and examples: [github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- Issues: [GitHub Issues](https://github.com/KenAKAFrosty/Prns/issues)
- Security reports: [Security policy](https://github.com/KenAKAFrosty/Prns/security/policy)

Packages are licensed under MIT or Apache-2.0, at your option.

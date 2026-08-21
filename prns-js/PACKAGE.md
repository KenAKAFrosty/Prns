# Personal RNS (Prns)

Prns is a safe, robust, fast Reticulum implementation with one language-neutral host contract. Rust and TypeScript/JavaScript are its paved application SDKs. Source-ready native SDK previews are implemented for Python, .NET, Go, Swift, Kotlin, Java, Julia, C, and C++ while their idiomatic public distribution is completed.

Every hosted SDK delegates protocol behavior to the same engine. Native FFI packages use the versioned C ABI; Node and Bun talk directly to the native Rust host; browsers run the cooperative engine through WebAssembly. Language packages own types, deterministic lifetime, cancellation, and ecosystem-native streams; they do not reimplement routing or wire semantics.

The package version and contract ABI are checked before host creation. Commands settle as typed success or failure values, event lanes have one explicit owner, and resource bodies retain their own bounded stream lifetime.

- Documentation: [reticulum.rs](https://reticulum.rs)
- Documentation mirror: [prns.dev](https://prns.dev)
- SDK readiness: [Prns SDK guide](https://github.com/KenAKAFrosty/Prns/blob/trunk/docs/sdks.md)
- Source and examples: [github.com/KenAKAFrosty/Prns](https://github.com/KenAKAFrosty/Prns)
- Issues: [GitHub Issues](https://github.com/KenAKAFrosty/Prns/issues)
- Security reports: [Security policy](https://github.com/KenAKAFrosty/Prns/security/policy)

Packages are licensed under MIT or Apache-2.0, at your option.

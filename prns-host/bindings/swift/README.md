# PersonalRns for Swift

> **Status: solid core, young surface.**
> This binding runs the same Rust engine as every Prns node and passes the same cross-language conformance suite on every release.
> The young part is the Swift-facing API. Its shape is a working first draft: a starting point, not the final word.
> If you are an experienced Swift developer and something here does not feel native, that is exactly the feedback we want. Issues and PRs on API design are among the most valuable contributions right now.

The Swift package is a thin adapter over the stable Personal RNS C capsule. The schema generates Swift enums with associated values for every command, outcome, application event, and diagnostic event. Native event lanes surface as single-iterator `AsyncSequence` values, resource bodies are asynchronous byte sequences, and native readiness resumes Swift continuations without occupying a dispatch worker. Task cancellation interrupts readiness directly.

Install the matching native capsule so `pkg-config personal-rns` resolves it, then add the package:

```swift
func run(_ host: Host) async throws {
    guard case .claimed(let events) = try host.claimApplicationEvents() else {
        return
    }
    Task {
        for try await event in events {
            handle(event)
        }
    }

    switch try await host.attachTcpClient(
        target: "127.0.0.1:4242",
        bitrate: .auto
    ) {
    case .succeeded(let outcome):
        handle(outcome)
    case .failed(let failure):
        handleFailure(failure)
    }
}
```

Swift Package Manager reads the native include and link paths from the same relocatable `personal-rns.pc` file shipped in every native archive.

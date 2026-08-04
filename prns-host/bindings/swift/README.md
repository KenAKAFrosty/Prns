# PersonalRns for Swift

> **SDK preview: implemented, tested in-tree, and awaiting polished distribution.**
> This adapter runs the same Rust engine as every Prns node and is exercised by the repository's registered live Swift conformance suite.
> Swift Package tagging, matching native archives, and public-package qualification are active release work, so a source checkout is currently the supported evaluation path.
> If you are experienced with Swift API or package design, help making this feel completely at home in Swift would be especially valuable.

The Swift package is a thin adapter over the stable Personal RNS C capsule. The schema generates Swift enums with associated values for every command, outcome, application event, and diagnostic event. Native event lanes surface as single-iterator `AsyncSequence` values, resource bodies are asynchronous byte sequences, and native readiness resumes Swift continuations without occupying a dispatch worker. Task cancellation interrupts readiness directly.

## Evaluate the current source

On Linux, the registered suite builds a relocatable native capsule, exposes its pkg-config metadata, and runs the complete persistent two-node journey:

```console
python3 validation/run.py run --suite host-swift-contract
```

The intended public delivery is an immutable Swift Package tag paired with a matching signed native archive. Until those artifacts have completed public qualification, do not assume the release tag exists. See the [SDK guide](../../../docs/sdks.md#native-sdk-previews) for the shared release posture and contribution path.

## API shape

With `pkg-config personal-rns` resolving the matching native capsule:

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

Swift Package Manager reads the native include and link paths from the same relocatable `personal-rns.pc` file intended for each native archive.

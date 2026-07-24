# PersonalRns for Swift

The Swift package is a thin adapter over the stable Personal RNS C capsule. The schema generates Swift enums with associated values for every command, outcome, application event, and diagnostic event. Native event lanes surface as single-iterator `AsyncSequence` values, resource bodies are asynchronous byte sequences, and task cancellation interrupts the underlying infinite native wait immediately.

Install the matching native capsule so `pkg-config personal-rns` resolves it, then add the package:

```swift
let host = try Host(
    options: .ephemeralEndpoint(requiredCapabilities: [.tcpClient])
)

if case .claimed(let events) = try host.claimApplicationEvents() {
    Task {
        for try await event in events {
            handle(event)
        }
    }
}

let command = try host.execute(
    .attachTcpClient(target: "127.0.0.1:4242", bitrate: .auto)
)
switch try await command.value() {
case .succeeded(let outcome):
    handle(outcome)
case .failed(let kind, let detail):
    handleFailure(kind, detail)
}
```

Swift Package Manager reads the native include and link paths from the same relocatable `personal-rns.pc` file shipped in every native archive.

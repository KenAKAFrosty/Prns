// A standalone consumer of the generated module and selected Rust native image.
import Foundation
import PrnsHostBindings

private enum ConformanceFailure: Error {
    case failed(String)
}

private func require(_ value: Bool, _ message: String) throws {
    if !value { throw ConformanceFailure.failed(message) }
}

@main
struct SwiftConformance {
    static func main() async throws {
        guard CommandLine.arguments.count == 3 else {
            throw ConformanceFailure.failed("expected canonical fingerprint and package version")
        }
        let contract = bindingContract()
        try require(contract.semanticFingerprint == CommandLine.arguments[1], "native contract fingerprint")
        try require(contract.packageVersion == CommandLine.arguments[2], "native package version")

        // Generated buffer converters use the linked Rust allocator. UInt64.max
        // must survive transport exactly, even where checked native admission
        // deliberately applies the canonical safe-integer constraint below.
        for role: HostRole in [.endpoint, .transport] {
            let restored = try FfiConverterTypeHostRole_lift(FfiConverterTypeHostRole_lower(role))
            try require(restored == role, "HostRole roundtrip")
        }
        for bitrate: Bitrate in [.auto, .bitsPerSecond(value: 0), .bitsPerSecond(value: UInt64.max)] {
            let restored = try FfiConverterTypeBitrate_lift(FfiConverterTypeBitrate_lower(bitrate))
            try require(restored == bitrate, "Bitrate/UInt64 roundtrip")
        }

        for role: HostRole in [.endpoint, .transport] {
            let config = HostConfig(
                identity: .generateEphemeral, persistence: .ephemeral, role: role,
                destinations: [], requiredCapabilities: [],
                limits: PrnsLimits(pendingCommands: 16, applicationEvents: 16,
                                  retainedEventBytes: 1_048_576, diagnostics: 16)
            )
            // Swift's async entrypoint has no ambient Tokio executor. The native
            // owner supplies its own runtime; the client is only a borrowed lease.
            let owner = try await openHost(config: config)
            let client = owner.client()
            do {
                try require(try client.identityHash().count == 16, "native identity")
                let initial = try await client.snapshot()
                try require(initial.interfaces.isEmpty, "new host has no interfaces")
                do {
                    _ = try await client.execute(command: .attachTcpServer(
                        bind: "127.0.0.1:0", bitrate: .bitsPerSecond(value: UInt64.max)
                    ))
                    throw ConformanceFailure.failed("unchecked native bitrate accepted")
                } catch BindingError.InvalidInput(let field, _) {
                    try require(field == "safeUint", "native checked-input field")
                }
                try require(try await client.snapshot().interfaces.isEmpty, "rejected input caused an effect")
                let retained = try RemoteControlOperations(capacity: 1)
                let operation = try retained.submitLink(client: client, linkId: Data(repeating: 0xA1, count: 16), request: .describe)
                guard let snapshot = await retained.wait(operationId: operation) else {
                    throw ConformanceFailure.failed("missing retained result")
                }
                guard case .notSubmitted = snapshot.state else {
                    throw ConformanceFailure.failed("invalid link must be positively rejected")
                }
                try require(snapshot.requestKind == .describe, "canonical request kind")
                try await owner.stop()
                try await owner.stop()
                try require(await retained.wait(operationId: operation) == snapshot, "result survives stop and waiter replacement")
                try require(retained.retire(operationId: operation) == .retired, "explicit retirement")
                try require(retained.operation(operationId: operation) == nil, "retired result removed")
                do {
                    _ = try await client.snapshot()
                    throw ConformanceFailure.failed("borrowed client remained usable after stop")
                } catch BindingError.Stopped {
                    // The borrowed handle keeps no independent runtime alive.
                }
            } catch {
                try? await owner.stop()
                throw error
            }
        }
        print("PRNS_NATIVE_HOST_SWIFT_OK: canonical enums, exact UInt64 transport, native fingerprint, checked input, open/borrow/stop")
    }
}

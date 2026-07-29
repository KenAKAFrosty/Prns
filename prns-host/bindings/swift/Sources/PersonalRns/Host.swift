import CPrnsHost
import Foundation

public enum StreamClaim<Stream: Sendable>: Sendable {
    case claimed(Stream)
    case alreadyClaimed
}

public final class Host: @unchecked Sendable {
    private let lock = NSLock()
    private var pointer: OpaquePointer?
    public let identityHash: IdentityHash
    public let destinationHashes: [DestinationHash]

    public init(options: HostOptions) throws {
        try verifyNativeContract()
        let arena = NativeArena()
        var nativeOptions = try nativeHostOptions(options, arena: arena)
        var nativePointer: OpaquePointer?
        try checkedStatus(
            prns_host_create(&nativeOptions, &nativePointer),
            operation: "createHost"
        )
        guard let nativePointer else {
            throw StatusFailure(operation: "createHost", status: .backendFailed)
        }
        let nativeIdentityHash: IdentityHash
        let nativeDestinationHashes: [DestinationHash]
        do {
            nativeIdentityHash = try Host.readIdentityHash(nativePointer)
            nativeDestinationHashes = try Host.readDestinationHashes(nativePointer)
        } catch {
            prns_host_release(nativePointer)
            throw error
        }
        pointer = nativePointer
        identityHash = nativeIdentityHash
        destinationHashes = nativeDestinationHashes
    }

    deinit {
        close()
    }

    private func withPointer<Value>(
        _ body: (OpaquePointer) throws -> Value
    ) throws -> Value {
        lock.lock()
        defer { lock.unlock() }
        guard let pointer else {
            throw StatusFailure(operation: "host", status: .stopped)
        }
        return try body(pointer)
    }

    private static func readIdentityHash(
        _ pointer: OpaquePointer
    ) throws -> IdentityHash {
        var view = PrnsByteView(data: nil, length: 0)
        try checkedStatus(
            prns_host_identity_hash(pointer, &view),
            operation: "identityHash"
        )
        return try IdentityHash(copyBytes(view))
    }

    private static func readDestinationHashes(
        _ pointer: OpaquePointer
    ) throws -> [DestinationHash] {
        let count = prns_host_destination_count(pointer)
        return try (0..<count).map { index in
            var view = PrnsByteView(data: nil, length: 0)
            try checkedStatus(
                prns_host_destination_hash(pointer, index, &view),
                operation: "destinationHash"
            )
            return try DestinationHash(copyBytes(view))
        }
    }

    public func execute(_ command: HostCommand) throws -> Command {
        try withPointer { pointer in
            try Command.submit(host: pointer, command: command)
        }
    }

    public func claimApplicationEvents() throws -> StreamClaim<EventSequence<ApplicationEvent>> {
        try withPointer { pointer in
            var stream: OpaquePointer?
            let status = Status(
                rawValue: prns_host_claim_application_events(pointer, &stream)
            )
            if status == .alreadyClaimed {
                return .alreadyClaimed
            }
            guard status == .ok, let stream else {
                throw StatusFailure(
                    operation: "claimApplicationEvents",
                    status: status ?? .backendFailed
                )
            }
            return .claimed(
                EventSequence(
                    native: try NativeEventStream(pointer: stream),
                    decode: decodeApplicationEvent
                )
            )
        }
    }

    public func claimDiagnostics() throws -> StreamClaim<EventSequence<DiagnosticEvent>> {
        try withPointer { pointer in
            var stream: OpaquePointer?
            let status = Status(
                rawValue: prns_host_claim_diagnostics(pointer, &stream)
            )
            if status == .alreadyClaimed {
                return .alreadyClaimed
            }
            guard status == .ok, let stream else {
                throw StatusFailure(
                    operation: "claimDiagnostics",
                    status: status ?? .backendFailed
                )
            }
            return .claimed(
                EventSequence(
                    native: try NativeEventStream(pointer: stream),
                    decode: decodeDiagnosticEvent
                )
            )
        }
    }

    public func stop() throws {
        try withPointer { pointer in
            let status = Status(rawValue: prns_host_stop(pointer))
            guard status == .ok || status == .stopped else {
                throw StatusFailure(
                    operation: "stopHost",
                    status: status ?? .backendFailed
                )
            }
        }
    }

    public func close() {
        lock.lock()
        let pointer = pointer
        self.pointer = nil
        lock.unlock()
        if let pointer {
            prns_host_release(pointer)
        }
    }
}

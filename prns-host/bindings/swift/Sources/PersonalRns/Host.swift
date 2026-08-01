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

    public var backendInfo: BackendInfo {
        get throws {
            try withPointer { _ in
                var value = PrnsBackendInfo()
                value.struct_size = MemoryLayout<PrnsBackendInfo>.size
                try checkedStatus(
                    prns_backend_info(&value),
                    operation: "backendInfo"
                )
                return try decodeBackendInfo(value)
            }
        }
    }

    public func snapshot(timeoutMillis: UInt32 = 5_000) throws -> HostSnapshot {
        try withPointer { pointer in
            var inspection: OpaquePointer?
            try checkedStatus(
                prns_host_snapshot(pointer, timeoutMillis, &inspection),
                operation: "captureSnapshot"
            )
            guard let inspection else {
                throw StatusFailure(
                    operation: "captureSnapshot",
                    status: .backendFailed
                )
            }
            defer { prns_host_snapshot_release(inspection) }
            var value = PrnsHostSnapshot()
            value.struct_size = MemoryLayout<PrnsHostSnapshot>.size
            try checkedStatus(
                prns_host_snapshot_read(inspection, &value),
                operation: "readSnapshot"
            )
            return try decodeHostSnapshot(value)
        }
    }

    public func beginResourceUpload(
        linkId: LinkId,
        declaredLength: UInt64,
        packedMetadata: [UInt8]? = nil,
        compression: ResourceCompression = .auto
    ) throws -> ResourceUpload {
        try withPointer { pointer in
            let arena = NativeArena()
            var output: OpaquePointer?
            let status: UInt32
            if let packedMetadata {
                var metadata = try arena.bytes(packedMetadata)
                status = prns_host_begin_resource_upload(
                    pointer,
                    try arena.bytes(linkId.bytes),
                    declaredLength,
                    &metadata,
                    compression.native,
                    &output
                )
            } else {
                status = prns_host_begin_resource_upload(
                    pointer,
                    try arena.bytes(linkId.bytes),
                    declaredLength,
                    nil,
                    compression.native,
                    &output
                )
            }
            try checkedStatus(status, operation: "beginResourceUpload")
            guard let output else {
                throw StatusFailure(
                    operation: "beginResourceUpload",
                    status: .backendFailed
                )
            }
            return ResourceUpload(pointer: output)
        }
    }

    public func sendResource(
        linkId: LinkId,
        payload: [UInt8],
        packedMetadata: [UInt8]? = nil,
        compression: ResourceCompression = .auto
    ) async throws -> CommandSettlement {
        let upload = try beginResourceUpload(
            linkId: linkId,
            declaredLength: UInt64(payload.count),
            packedMetadata: packedMetadata,
            compression: compression
        )
        do {
            try await upload.write(payload)
            return try await upload.finish()
        } catch {
            upload.abort()
            upload.close()
            throw error
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

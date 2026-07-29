import CPrnsHost
import Foundation

final class NativeArena {
    private struct Allocation {
        let pointer: UnsafeMutableRawPointer
        let size: Int
    }

    private var allocations: [Allocation] = []

    func allocate(byteCount: Int, alignment: Int) throws -> UnsafeMutableRawPointer {
        let size = max(byteCount, 1)
        let pointer = UnsafeMutableRawPointer.allocate(
            byteCount: size,
            alignment: alignment
        )
        pointer.initializeMemory(as: UInt8.self, repeating: 0, count: size)
        allocations.append(Allocation(pointer: pointer, size: size))
        return pointer
    }

    func bytes(_ value: [UInt8], preserveEmpty: Bool = false) throws -> PrnsByteView {
        if value.isEmpty && !preserveEmpty {
            return PrnsByteView(data: nil, length: 0)
        }
        let pointer = try allocate(
            byteCount: max(value.count, 1),
            alignment: MemoryLayout<UInt8>.alignment
        )
        value.withUnsafeBytes { source in
            guard let baseAddress = source.baseAddress, !value.isEmpty else {
                return
            }
            pointer.copyMemory(from: baseAddress, byteCount: value.count)
        }
        return PrnsByteView(
            data: UnsafePointer(pointer.assumingMemoryBound(to: UInt8.self)),
            length: value.count
        )
    }

    func optionalBytes(_ value: [UInt8]?) throws -> PrnsByteView {
        guard let value else {
            return PrnsByteView(data: nil, length: 0)
        }
        return try bytes(value, preserveEmpty: true)
    }

    func string(_ value: String) throws -> PrnsStringView {
        let view = try bytes(Array(value.utf8))
        return PrnsStringView(data: view.data, length: view.length)
    }

    func array<Element>(_ values: [Element]) throws -> UnsafeMutablePointer<Element>? {
        guard !values.isEmpty else {
            return nil
        }
        let raw = try allocate(
            byteCount: MemoryLayout<Element>.stride * values.count,
            alignment: MemoryLayout<Element>.alignment
        )
        let pointer = raw.bindMemory(to: Element.self, capacity: values.count)
        for (index, value) in values.enumerated() {
            pointer.advanced(by: index).initialize(to: value)
        }
        return pointer
    }

    deinit {
        for allocation in allocations.reversed() {
            allocation.pointer.initializeMemory(
                as: UInt8.self,
                repeating: 0,
                count: allocation.size
            )
            allocation.pointer.deallocate()
        }
    }
}

func nativeIdentity(
    _ value: IdentityConfig,
    arena: NativeArena
) throws -> PrnsIdentityConfig {
    switch value {
    case .existing(let secret):
        let view = try secret.withUnsafeBytes { bytes in
            try arena.bytes(Array(bytes))
        }
        return PrnsIdentityConfig(
            struct_size: MemoryLayout<PrnsIdentityConfig>.size,
            kind: IdentityConfigKind.existing.rawValue,
            secret: view,
            path: PrnsStringView(data: nil, length: 0)
        )
    case .generateEphemeral:
        return PrnsIdentityConfig(
            struct_size: MemoryLayout<PrnsIdentityConfig>.size,
            kind: IdentityConfigKind.generateEphemeral.rawValue,
            secret: PrnsByteView(data: nil, length: 0),
            path: PrnsStringView(data: nil, length: 0)
        )
    case .loadOrCreate(let path):
        return PrnsIdentityConfig(
            struct_size: MemoryLayout<PrnsIdentityConfig>.size,
            kind: IdentityConfigKind.loadOrCreate.rawValue,
            secret: PrnsByteView(data: nil, length: 0),
            path: try arena.string(path)
        )
    }
}

func nativeDestinationName(
    _ value: DestinationName,
    arena: NativeArena
) throws -> PrnsDestinationName {
    let aspects = try value.aspects.map { try arena.string($0) }
    return PrnsDestinationName(
        struct_size: MemoryLayout<PrnsDestinationName>.size,
        app_name: try arena.string(value.appName),
        aspects: try arena.array(aspects).map { UnsafePointer($0) },
        aspect_count: aspects.count
    )
}

func emptyNativeIdentity() -> PrnsIdentityConfig {
    PrnsIdentityConfig(
        struct_size: MemoryLayout<PrnsIdentityConfig>.size,
        kind: 0,
        secret: PrnsByteView(data: nil, length: 0),
        path: PrnsStringView(data: nil, length: 0)
    )
}

func nativeDestinationIdentity(
    _ value: DestinationIdentityConfig,
    arena: NativeArena
) throws -> (UInt32, PrnsIdentityConfig) {
    switch value {
    case .hostIdentity:
        return (
            DestinationIdentityConfigKind.hostIdentity.rawValue,
            emptyNativeIdentity()
        )
    case .dedicatedIdentity(let identity):
        return (
            DestinationIdentityConfigKind.dedicatedIdentity.rawValue,
            try nativeIdentity(identity, arena: arena)
        )
    }
}

func nativeDestination(
    _ value: DestinationConfig,
    arena: NativeArena
) throws -> PrnsDestinationConfig {
    switch value {
    case .plain(let name):
        return PrnsDestinationConfig(
            struct_size: MemoryLayout<PrnsDestinationConfig>.size,
            kind: DestinationConfigKind.plain.rawValue,
            name: try nativeDestinationName(name, arena: arena),
            identity_kind: 0,
            dedicated_identity: emptyNativeIdentity(),
            announce_app_data: PrnsByteView(data: nil, length: 0),
            request_handlers: nil,
            request_handler_count: 0
        )
    case .single(let name, let identity, let announceAppData, let requestHandlers):
        let nativeIdentity = try nativeDestinationIdentity(identity, arena: arena)
        let handlers = try requestHandlers.map { handler in
            PrnsRequestHandlerConfig(
                struct_size: MemoryLayout<PrnsRequestHandlerConfig>.size,
                path: try arena.string(handler.path),
                policy: handler.policy.rawValue
            )
        }
        return PrnsDestinationConfig(
            struct_size: MemoryLayout<PrnsDestinationConfig>.size,
            kind: DestinationConfigKind.single.rawValue,
            name: try nativeDestinationName(name, arena: arena),
            identity_kind: nativeIdentity.0,
            dedicated_identity: nativeIdentity.1,
            announce_app_data: try arena.optionalBytes(announceAppData),
            request_handlers: try arena.array(handlers).map { UnsafePointer($0) },
            request_handler_count: handlers.count
        )
    }
}

func nativeHostOptions(
    _ value: HostOptions,
    arena: NativeArena
) throws -> PrnsHostOptions {
    let destinations = try value.destinations.map {
        try nativeDestination($0, arena: arena)
    }
    let capabilities = value.requiredCapabilities.map(\.rawValue)
    return PrnsHostOptions(
        struct_size: MemoryLayout<PrnsHostOptions>.size,
        required_abi: HostContract.abi,
        required_product_version: try arena.string(HostContract.productVersion),
        limits: PrnsLimits(
            struct_size: MemoryLayout<PrnsLimits>.size,
            pending_commands: value.limits.pendingCommands,
            application_events: value.limits.applicationEvents,
            retained_event_bytes: value.limits.retainedEventBytes,
            diagnostics: value.limits.diagnostics
        ),
        role: value.role.rawValue,
        identity: try nativeIdentity(value.identity, arena: arena),
        destinations: try arena.array(destinations).map { UnsafePointer($0) },
        destination_count: destinations.count,
        required_capabilities: try arena.array(capabilities).map { UnsafePointer($0) },
        required_capability_count: capabilities.count
    )
}

func copyBytes(_ view: PrnsByteView) -> [UInt8] {
    guard let data = view.data, view.length > 0 else {
        return []
    }
    return Array(UnsafeBufferPointer(start: data, count: view.length))
}

func copyString(_ view: PrnsStringView) -> String {
    String(decoding: copyBytes(PrnsByteView(data: view.data, length: view.length)), as: UTF8.self)
}

public struct StatusFailure: Error, Equatable, Sendable {
    public let operation: String
    public let status: Status

    public init(operation: String, status: Status) {
        self.operation = operation
        self.status = status
    }
}

func checkedStatus(_ rawValue: UInt32, operation: String) throws {
    guard let status = Status(rawValue: rawValue) else {
        throw StatusFailure(operation: operation, status: .backendFailed)
    }
    guard status == .ok else {
        throw StatusFailure(operation: operation, status: status)
    }
}

func verifyNativeContract() throws {
    var info = PrnsContractInfo(
        struct_size: MemoryLayout<PrnsContractInfo>.size,
        abi: 0,
        schema_version: 0,
        product_version: PrnsStringView(data: nil, length: 0)
    )
    try checkedStatus(prns_contract_info(&info), operation: "contractInfo")
    guard info.abi == HostContract.abi,
          info.schema_version == HostContract.schemaVersion,
          copyString(info.product_version) == HostContract.productVersion
    else {
        throw StatusFailure(operation: "contractInfo", status: .contractMismatch)
    }
}

private func signalNativeReadiness(
    _ context: UnsafeMutableRawPointer?
) {
    guard let context else {
        return
    }
    Unmanaged<NativeReadiness>
        .fromOpaque(context)
        .takeUnretainedValue()
        .signal()
}

final class NativeReadiness: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [CheckedContinuation<Void, Never>] = []
    private var pending = false
    private var registration: OpaquePointer?

    private init() {}

    static func command(_ command: OpaquePointer) throws -> NativeReadiness {
        let readiness = NativeReadiness()
        var registration: OpaquePointer?
        try checkedStatus(
            prns_command_register_readiness(
                command,
                signalNativeReadiness,
                Unmanaged.passUnretained(readiness).toOpaque(),
                &registration
            ),
            operation: "registerCommandReadiness"
        )
        guard let registration else {
            throw StatusFailure(
                operation: "registerCommandReadiness",
                status: .backendFailed
            )
        }
        readiness.registration = registration
        return readiness
    }

    static func eventStream(_ stream: OpaquePointer) throws -> NativeReadiness {
        let readiness = NativeReadiness()
        var registration: OpaquePointer?
        try checkedStatus(
            prns_event_stream_register_readiness(
                stream,
                signalNativeReadiness,
                Unmanaged.passUnretained(readiness).toOpaque(),
                &registration
            ),
            operation: "registerEventReadiness"
        )
        guard let registration else {
            throw StatusFailure(
                operation: "registerEventReadiness",
                status: .backendFailed
            )
        }
        readiness.registration = registration
        return readiness
    }

    func signal() {
        lock.lock()
        let continuations = continuations
        self.continuations.removeAll()
        if continuations.isEmpty {
            pending = true
        }
        lock.unlock()
        for continuation in continuations {
            continuation.resume()
        }
    }

    func wait() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if pending || registration == nil {
                pending = false
                lock.unlock()
                continuation.resume()
            } else {
                continuations.append(continuation)
                lock.unlock()
            }
        }
    }

    func close() {
        lock.lock()
        let registration = registration
        self.registration = nil
        let continuations = continuations
        self.continuations.removeAll()
        lock.unlock()
        if let registration {
            prns_readiness_registration_release(registration)
        }
        for continuation in continuations {
            continuation.resume()
        }
    }
}

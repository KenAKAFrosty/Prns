import CPrnsHost
import Foundation

public enum CommandSettlement: Sendable {
    case succeeded(CommandOutcome)
    case failed(kind: CommandFailureKind, detail: String)
}

public final class Command: @unchecked Sendable {
    private let stateLock = NSLock()
    private let waitLock = NSLock()
    private var pointer: OpaquePointer?

    init(pointer: OpaquePointer) {
        self.pointer = pointer
    }

    deinit {
        close()
    }

    static func submit(
        host: OpaquePointer,
        command: HostCommand
    ) throws -> Command {
        let arena = NativeArena()
        var output: OpaquePointer?
        let status: UInt32
        switch command {
        case .announce(let destination, let interface):
            let nativeDestination = try arena.bytes(destination.bytes)
            if let interface {
                var nativeInterface = try arena.bytes(interface.bytes)
                status = prns_host_announce(
                    host,
                    nativeDestination,
                    &nativeInterface,
                    &output
                )
            } else {
                status = prns_host_announce(
                    host,
                    nativeDestination,
                    nil,
                    &output
                )
            }
        case .sendSinglePacket(let destination, let payload):
            status = prns_host_send_single_packet(
                host,
                try arena.bytes(destination.bytes),
                try arena.bytes(payload),
                &output
            )
        case .closeLink(let linkId):
            status = prns_host_close_link(
                host,
                try arena.bytes(linkId.bytes),
                &output
            )
        case .attachTcpServer(let bind, let bitrate):
            let nativeBitrate = try bitrate.native
            status = prns_host_attach_tcp_server(
                host,
                try arena.string(bind),
                nativeBitrate.kind,
                nativeBitrate.bitsPerSecond,
                &output
            )
        case .attachTcpClient(let target, let bitrate):
            let nativeBitrate = try bitrate.native
            status = prns_host_attach_tcp_client(
                host,
                try arena.string(target),
                nativeBitrate.kind,
                nativeBitrate.bitsPerSecond,
                &output
            )
        case .attachUdp(let local, let peer, let bitrate):
            let nativeBitrate = try bitrate.native
            status = prns_host_attach_udp(
                host,
                try arena.string(local),
                try arena.string(peer),
                nativeBitrate.kind,
                nativeBitrate.bitsPerSecond,
                &output
            )
        case .detachInterface(let interface):
            status = prns_host_detach_interface(
                host,
                try arena.bytes(interface.bytes),
                &output
            )
        }
        try checkedStatus(status, operation: "submitCommand")
        guard let output else {
            throw StatusFailure(operation: "submitCommand", status: .backendFailed)
        }
        return Command(pointer: output)
    }

    private func snapshot() throws -> OpaquePointer {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard let pointer else {
            throw StatusFailure(operation: "command", status: .stopped)
        }
        return pointer
    }

    private func interruptWait() {
        stateLock.lock()
        if let pointer {
            prns_command_interrupt_wait(pointer)
        }
        stateLock.unlock()
    }

    public func value() async throws -> CommandSettlement {
        return try await asyncNative {
            self.interruptWait()
        } operation: {
            self.waitLock.lock()
            defer { self.waitLock.unlock() }
            let pointer = try self.snapshot()
            var result = PrnsCommandResult(
                struct_size: MemoryLayout<PrnsCommandResult>.size,
                outcome: 0,
                failure: 0,
                evidence: 0,
                rtt_millis: 0,
                value: PrnsByteView(data: nil, length: 0),
                detail: PrnsStringView(data: nil, length: 0)
            )
            let status = Status(
                rawValue: prns_command_wait(
                    pointer,
                    nativeNeverTimeout,
                    &result
                )
            )
            if status == .interrupted {
                throw CancellationError()
            }
            guard status == .ok else {
                throw StatusFailure(
                    operation: "waitCommand",
                    status: status ?? .backendFailed
                )
            }
            return try Command.decode(result)
        }
    }

    private static func decode(
        _ value: PrnsCommandResult
    ) throws -> CommandSettlement {
        if value.failure != 0 {
            guard let failure = CommandFailureKind(rawValue: value.failure) else {
                throw StatusFailure(
                    operation: "decodeCommand",
                    status: .backendFailed
                )
            }
            return .failed(kind: failure, detail: copyString(value.detail))
        }
        guard let outcome = CommandOutcomeKind(rawValue: value.outcome) else {
            throw StatusFailure(
                operation: "decodeCommand",
                status: .backendFailed
            )
        }
        switch outcome {
        case .announced:
            return .succeeded(.announced)
        case .packetDelivered:
            guard let evidence = DeliveryEvidenceKind(
                rawValue: value.evidence
            ) else {
                throw StatusFailure(
                    operation: "decodeCommand",
                    status: .backendFailed
                )
            }
            let bytes = copyBytes(value.value)
            let packetHash: PacketHash?
            switch evidence {
            case .response:
                guard bytes.isEmpty else {
                    throw StatusFailure(
                        operation: "decodeResponseEvidence",
                        status: .backendFailed
                    )
                }
                packetHash = nil
            case .explicitProof, .implicitProof:
                packetHash = try PacketHash(bytes)
            }
            return .succeeded(
                .packetDelivered(
                    rttMillis: value.rtt_millis,
                    evidence: evidence,
                    packetHash: packetHash
                )
            )
        case .linkCloseQueued:
            return .succeeded(.linkCloseQueued)
        case .interfaceAttached:
            return .succeeded(
                .interfaceAttached(
                    interface: try InterfaceId(copyBytes(value.value))
                )
            )
        case .interfaceDetached:
            return .succeeded(
                .interfaceDetached(
                    interface: try InterfaceId(copyBytes(value.value))
                )
            )
        }
    }

    public func close() {
        stateLock.lock()
        let pointer = pointer
        self.pointer = nil
        if let pointer {
            prns_command_interrupt_wait(pointer)
        }
        stateLock.unlock()
        guard let pointer else {
            return
        }
        waitLock.lock()
        prns_command_release(pointer)
        waitLock.unlock()
    }
}

private extension Bitrate {
    var native: (
        kind: UInt32,
        bitsPerSecond: UInt64
    ) {
        get throws {
            switch self {
            case .auto:
                return (BitrateKind.auto.rawValue, 0)
            case .bitsPerSecond(let value):
                return (BitrateKind.bitsPerSecond.rawValue, value)
            }
        }
    }
}

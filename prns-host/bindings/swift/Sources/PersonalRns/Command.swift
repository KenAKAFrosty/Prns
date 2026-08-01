import CPrnsHost
import Foundation

public enum CommandSettlement: Sendable {
    case succeeded(CommandOutcome)
    case failed(CommandFailure)
}

public final class Command: @unchecked Sendable {
    private let stateLock = NSLock()
    private let waitLock = NSLock()
    private let readiness: NativeReadiness
    private var pointer: OpaquePointer?

    init(pointer: OpaquePointer) throws {
        do {
            readiness = try NativeReadiness.command(pointer)
        } catch {
            prns_command_release(pointer)
            throw error
        }
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
        case .attachInterface(let config):
            var nativeConfig = try nativeInterfaceConfig(config, arena: arena)
            status = prns_host_attach_interface(
                host,
                &nativeConfig,
                &output
            )
        case .detachInterface(let interface):
            status = prns_host_detach_interface(
                host,
                try arena.bytes(interface.bytes),
                &output
            )
        case .establishLink(let destination):
            status = prns_host_establish_link(
                host,
                try arena.bytes(destination.bytes),
                &output
            )
        case .requestPath(let destination):
            status = prns_host_request_path(
                host,
                try arena.bytes(destination.bytes),
                &output
            )
        case .identify(let linkId, let identity):
            status = prns_host_identify(
                host,
                try arena.bytes(linkId.bytes),
                try arena.bytes(identity.bytes),
                &output
            )
        case .sendLinkPacket(let linkId, let payload):
            status = prns_host_send_link_packet(
                host,
                try arena.bytes(linkId.bytes),
                try arena.bytes(payload),
                &output
            )
        case .request(let linkId, let pathHash, let payload, let timeout):
            let nativeTimeout = timeout.native
            status = prns_host_request(
                host,
                try arena.bytes(linkId.bytes),
                try arena.bytes(pathHash.bytes),
                try arena.bytes(payload),
                nativeTimeout.kind,
                nativeTimeout.millis,
                &output
            )
        case .respond(
            let linkId,
            let requestId,
            let requestRttMillis,
            let payload
        ):
            status = prns_host_respond(
                host,
                try arena.bytes(linkId.bytes),
                try arena.bytes(requestId.bytes),
                requestRttMillis,
                try arena.bytes(payload),
                &output
            )
        case .sendResource(
            let linkId,
            let payload,
            let packedMetadata,
            let compression
        ):
            if let packedMetadata {
                var nativeMetadata = try arena.bytes(packedMetadata)
                status = prns_host_send_resource(
                    host,
                    try arena.bytes(linkId.bytes),
                    try arena.bytes(payload),
                    &nativeMetadata,
                    compression.native,
                    &output
                )
            } else {
                status = prns_host_send_resource(
                    host,
                    try arena.bytes(linkId.bytes),
                    try arena.bytes(payload),
                    nil,
                    compression.native,
                    &output
                )
            }
        case .setLinkResourceStrategy(let linkId, let strategy):
            let nativeStrategy = try strategy.native
            status = prns_host_set_link_resource_strategy(
                host,
                try arena.bytes(linkId.bytes),
                nativeStrategy.kind,
                nativeStrategy.maximum,
                nativeStrategy.acceptCompressed,
                &output
            )
        case .setDestinationResourceStrategy(let destination, let strategy):
            let nativeStrategy = try strategy.native
            status = prns_host_set_destination_resource_strategy(
                host,
                try arena.bytes(destination.bytes),
                nativeStrategy.kind,
                nativeStrategy.maximum,
                nativeStrategy.acceptCompressed,
                &output
            )
        case .sendChannelMessage(let linkId, let messageType, let payload):
            status = prns_host_send_channel_message(
                host,
                try arena.bytes(linkId.bytes),
                messageType,
                try arena.bytes(payload),
                &output
            )
        case .allowRequester(let destination, let pathHash, let identity):
            status = prns_host_allow_requester(
                host,
                try arena.bytes(destination.bytes),
                try arena.bytes(pathHash.bytes),
                try arena.bytes(identity.bytes),
                &output
            )
        }
        try checkedStatus(status, operation: "submitCommand")
        guard let output else {
            throw StatusFailure(operation: "submitCommand", status: .backendFailed)
        }
        return try Command(pointer: output)
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
        return try await withTaskCancellationHandler {
            while true {
                if let settlement = try poll() {
                    return settlement
                }
                await readiness.wait()
            }
        } onCancel: {
            self.interruptWait()
        }
    }

    private func poll() throws -> CommandSettlement? {
        try waitLock.withLock {
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
                    0,
                    &result
                )
            )
            if status == .timedOut {
                return nil
            }
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
            return .failed(
                decodeCommandFailure(failure, detail: copyString(value.detail))
            )
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
        case .linkEstablished:
            return .succeeded(
                .linkEstablished(
                    linkId: try LinkId(copyBytes(value.value)),
                    rttMillis: value.rtt_millis
                )
            )
        case .pathDiscovered:
            let bytes = copyBytes(value.value)
            guard bytes.count == 1 else {
                throw StatusFailure(
                    operation: "decodePathDiscovery",
                    status: .backendFailed
                )
            }
            return .succeeded(.pathDiscovered(hops: bytes[0]))
        case .identified:
            return .succeeded(.identified)
        case .responseReceived:
            return .succeeded(
                .responseReceived(
                    data: copyBytes(value.value),
                    rttMillis: value.rtt_millis
                )
            )
        case .responseSent:
            return .succeeded(.responseSent(rttMillis: value.rtt_millis))
        case .resourceSent:
            return .succeeded(.resourceSent)
        case .resourceStrategySet:
            return .succeeded(.resourceStrategySet)
        case .requesterAllowed:
            return .succeeded(.requesterAllowed)
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
        waitLock.withLock {
            readiness.close()
            prns_command_release(pointer)
        }
    }
}

extension Bitrate {
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

private func decodeCommandFailure(
    _ kind: CommandFailureKind,
    detail: String
) -> CommandFailure {
    switch kind {
    case .nodeStopped:
        return .nodeStopped
    case .busy:
        return .busy
    case .payloadTooLarge:
        return .payloadTooLarge
    case .unknownDestination:
        return .unknownDestination
    case .notSingleDestination:
        return .notSingleDestination
    case .announceAppDataTooLong:
        return .announceAppDataTooLong
    case .unknownInterface:
        return .unknownInterface
    case .noRouteToDestination:
        return .noRouteToDestination
    case .notDirectlyReachable:
        return .notDirectlyReachable
    case .packetCulled:
        return .packetCulled
    case .deliveryTimedOut:
        return .deliveryTimedOut
    case .invalidBitrate:
        return .invalidBitrate
    case .bindFailed:
        return .bindFailed(detail: detail)
    case .writeFailed:
        return .writeFailed(detail: detail)
    case .unsupportedByBackend:
        return .unsupportedByBackend
    case .unknownLink:
        return .unknownLink
    case .linkNotActive:
        return .linkNotActive
    case .entropyUnavailable:
        return .entropyUnavailable
    case .notLinkInitiator:
        return .notLinkInitiator
    case .identityNotHeld:
        return .identityNotHeld
    case .unknownRequestHandler:
        return .unknownRequestHandler
    case .requestPolicyNotAllowList:
        return .requestPolicyNotAllowList
    case .requestAllowListFull:
        return .requestAllowListFull
    case .linkBusy:
        return .linkBusy
    case .resourceTableFull:
        return .resourceTableFull
    case .resourceMetadataTooLarge:
        return .resourceMetadataTooLarge
    case .resourceRejectedByPeer:
        return .resourceRejectedByPeer
    case .resourceSequencingFailed:
        return .resourceSequencingFailed
    case .resourcePredecessorFailed:
        return .resourcePredecessorFailed
    case .channelWindowFull:
        return .channelWindowFull
    case .channelUntrackable:
        return .channelUntrackable
    case .invalidChannelMessageType:
        return .invalidChannelMessageType
    case .invalidConfiguration:
        return .invalidConfiguration(detail: detail)
    case .resourceUploadCancelled:
        return .resourceUploadCancelled
    case .resourceEarlyEof:
        return .resourceEarlyEof
    case .resourceLengthOverrun:
        return .resourceLengthOverrun
    case .permissionDenied:
        return .permissionDenied(detail: detail)
    case .deviceUnavailable:
        return .deviceUnavailable(detail: detail)
    case .connectFailed:
        return .connectFailed(detail: detail)
    case .backendFailed:
        return .backendFailed(detail: detail)
    }
}

private extension ResponseTimeout {
    var native: (kind: UInt32, millis: UInt64) {
        switch self {
        case .linkDefault:
            return (ResponseTimeoutKind.linkDefault.rawValue, 0)
        case .exact(let millis):
            return (ResponseTimeoutKind.exact.rawValue, millis)
        }
    }
}

extension ResourceCompression {
    var native: UInt32 {
        switch self {
        case .auto:
            return ResourceCompressionKind.auto.rawValue
        case .never:
            return ResourceCompressionKind.never.rawValue
        }
    }
}

private extension ResourceStrategy {
    var native: (
        kind: UInt32,
        maximum: UInt64,
        acceptCompressed: UInt8
    ) {
        get throws {
            switch self {
            case .refuse:
                return (ResourceStrategyKind.refuse.rawValue, 0, 0)
            case .accept(
                let maximumUncompressedBytes,
                let acceptCompressed
            ):
                guard maximumUncompressedBytes > 0 else {
                    throw StatusFailure(
                        operation: "marshalResourceStrategy",
                        status: .invalidArgument
                    )
                }
                return (
                    ResourceStrategyKind.accept.rawValue,
                    maximumUncompressedBytes,
                    acceptCompressed ? 1 : 0
                )
            }
        }
    }
}

import Foundation

public enum HostContract {
    public static let abi: UInt32 = 1
    public static let schemaVersion: UInt32 = 1
    public static let productVersion = "0.2.8"
    public static let destinationHashLength = 16
    public static let identityHashLength = 16
    public static let interfaceIdLength = 8
    public static let linkIdLength = 16
    public static let packetHashLength = 32
    public static let requestIdLength = 16
    public static let requestPathHashLength = 16
    public static let resourceHashLength = 32
    public static let identitySecretLength = 64
    public static let balancedPendingCommands = 256
    public static let balancedApplicationEvents = 1024
    public static let balancedRetainedEventBytes = 8388608
    public static let balancedDiagnostics = 1024
}

public enum Status: UInt32, Sendable {
    case ok = 0
    case invalidArgument = 1
    case contractMismatch = 2
    case invalidHandle = 3
    case notReady = 4
    case alreadyClaimed = 5
    case wouldBlock = 6
    case timedOut = 7
    case queueFull = 8
    case stopped = 9
    case backendFailed = 10
    case panic = 11
    case interrupted = 12
}

public enum BackendKind: UInt32, Sendable {
    case native = 1
    case browser = 2
    case cooperative = 3
}

public enum Capability: UInt32, Sendable {
    case loopback = 1
    case tcpClient = 2
    case tcpServer = 3
    case udp = 4
    case serial = 5
    case usb = 6
    case bluetooth = 7
    case wifi = 8
    case webSocket = 9
    case browserRendezvous = 10
    case i2p = 11
    case weave = 12
}

public enum HostRole: UInt32, Sendable {
    case endpoint = 1
    case transport = 2
}

public enum IdentityConfigKind: UInt32, Sendable {
    case existing = 1
    case generateEphemeral = 2
    case loadOrCreate = 3
}

public enum DestinationConfigKind: UInt32, Sendable {
    case plain = 1
    case single = 2
}

public enum DestinationIdentityConfigKind: UInt32, Sendable {
    case hostIdentity = 1
    case dedicatedIdentity = 2
}

public enum BitrateKind: UInt32, Sendable {
    case auto = 1
    case bitsPerSecond = 2
}

public enum CommandOutcomeKind: UInt32, Sendable {
    case announced = 1
    case packetDelivered = 2
    case linkCloseQueued = 3
    case interfaceAttached = 4
    case interfaceDetached = 5
}

public enum CommandFailureKind: UInt32, Sendable {
    case nodeStopped = 1
    case busy = 2
    case payloadTooLarge = 3
    case unknownDestination = 4
    case notSingleDestination = 5
    case announceAppDataTooLong = 6
    case unknownInterface = 7
    case noRouteToDestination = 8
    case notDirectlyReachable = 9
    case packetCulled = 10
    case deliveryTimedOut = 11
    case invalidBitrate = 12
    case bindFailed = 13
    case writeFailed = 14
}

public enum DeliveryEvidenceKind: UInt32, Sendable {
    case explicitProof = 1
    case implicitProof = 2
    case response = 3
}

public enum LifecyclePhase: UInt32, Sendable {
    case starting = 1
    case running = 2
    case stopping = 3
    case stopped = 4
    case failed = 5
}

public enum StopReason: UInt32, Sendable {
    case requested = 1
    case backendExited = 2
}

public enum LinkClosedReason: UInt32, Sendable {
    case timeout = 1
    case peerClosed = 2
    case malformedRtt = 3
}

public enum ApplicationEventKind: UInt32, Sendable {
    case singleDelivery = 100
    case request = 101
    case response = 102
    case responseSegment = 103
    case resourceAvailable = 104
    case resourceSegment = 105
    case resourceNeedsDecompression = 106
    case channelMessage = 107
}

public enum DiagnosticEventKind: UInt32, Sendable {
    case announceHeard = 200
    case linkEstablished = 201
    case peerIdentified = 202
    case linkClosed = 203
    case linkInterfaceMismatch = 204
    case resourceAssembled = 205
    case resourceFailed = 206
    case resourceSendProgress = 207
    case selfRatchetRotated = 208
    case announceHeldDropped = 209
    case delivered = 210
    case routeExpired = 211
    case routeEvicted = 212
    case routeInterfaceGone = 213
    case routeDropped = 214
    case backendDiagnostic = 215
    case diagnosticsDropped = 216
}

public enum EventField: UInt32, Sendable {
    case destination = 1
    case sourceInterface = 2
    case plaintext = 3
    case linkId = 4
    case requestId = 5
    case requester = 6
    case pathHash = 7
    case rttMillis = 8
    case data = 9
    case segmentIndex = 10
    case totalSegments = 11
    case hash = 12
    case originalHash = 13
    case metadata = 14
    case totalBytes = 15
    case streamId = 16
    case uncompressedDataBytes = 17
    case messageType = 18
    case identity = 19
    case reason = 20
    case attachedInterface = 21
    case arrivedOn = 22
    case totalSizeBytes = 23
    case cause = 24
    case transferredBytes = 25
    case physicalTransferredBytes = 26
    case detail = 27
    case kind = 28
    case droppedCount = 29
    case hops = 30
    case stream = 31
}

public struct DestinationHash: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.destinationHashLength else {
            throw ContractValueError.invalidLength(type: "DestinationHash", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct IdentityHash: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.identityHashLength else {
            throw ContractValueError.invalidLength(type: "IdentityHash", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct InterfaceId: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.interfaceIdLength else {
            throw ContractValueError.invalidLength(type: "InterfaceId", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct LinkId: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.linkIdLength else {
            throw ContractValueError.invalidLength(type: "LinkId", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct PacketHash: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.packetHashLength else {
            throw ContractValueError.invalidLength(type: "PacketHash", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct RequestId: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.requestIdLength else {
            throw ContractValueError.invalidLength(type: "RequestId", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct RequestPathHash: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.requestPathHashLength else {
            throw ContractValueError.invalidLength(type: "RequestPathHash", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public struct ResourceHash: Hashable, Sendable {
    public let bytes: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.resourceHashLength else {
            throw ContractValueError.invalidLength(type: "ResourceHash", actual: bytes.count)
        }
        self.bytes = bytes
    }
}

public final class IdentitySecret: @unchecked Sendable {
    private var storage: [UInt8]

    public init(_ bytes: [UInt8]) throws {
        guard bytes.count == HostContract.identitySecretLength else {
            throw ContractValueError.invalidLength(type: "IdentitySecret", actual: bytes.count)
        }
        storage = bytes
    }

    public func withUnsafeBytes<Result>(
        _ body: (UnsafeRawBufferPointer) throws -> Result
    ) rethrows -> Result {
        try storage.withUnsafeBytes(body)
    }

    public func close() {
        _ = storage.withUnsafeMutableBytes { bytes in
            bytes.initializeMemory(as: UInt8.self, repeating: 0)
        }
    }

    deinit {
        close()
    }
}

public enum ContractValueError: Error, Equatable {
    case invalidLength(type: String, actual: Int)
}

public struct DestinationName: Hashable, Sendable {
    public let appName: String
    public let aspects: [String]

    public init(appName: String, aspects: [String]) {
        self.appName = appName
        self.aspects = aspects
    }
}

public protocol ResourceStream: AnyObject, AsyncSequence, Sendable
where Element == [UInt8] {
    var totalBytes: UInt64 { get }
    func close()
}

public enum IdentityConfig: Sendable {
    case existing(secret: IdentitySecret)
    case generateEphemeral
    case loadOrCreate(path: String)
}

public enum DestinationIdentityConfig: Sendable {
    case hostIdentity
    case dedicatedIdentity(identity: IdentityConfig)
}

public enum Bitrate: Sendable {
    case auto
    case bitsPerSecond(value: UInt64)
}

public enum DestinationConfig: Sendable {
    case plain(name: DestinationName)
    case single(name: DestinationName, identity: DestinationIdentityConfig, announceAppData: [UInt8]?)
}

public enum HostCommand: Sendable {
    case announce(destination: DestinationHash, interface: InterfaceId?)
    case sendSinglePacket(destination: DestinationHash, payload: [UInt8])
    case closeLink(linkId: LinkId)
    case attachTcpServer(bind: String, bitrate: Bitrate)
    case attachTcpClient(target: String, bitrate: Bitrate)
    case attachUdp(local: String, peer: String, bitrate: Bitrate)
    case detachInterface(interface: InterfaceId)
}

public enum CommandOutcome: Sendable {
    case announced
    case packetDelivered(rttMillis: UInt64, evidence: DeliveryEvidenceKind, packetHash: PacketHash?)
    case linkCloseQueued
    case interfaceAttached(interface: InterfaceId)
    case interfaceDetached(interface: InterfaceId)
}

public enum ApplicationEvent: Sendable {
    case singleDelivery(destination: DestinationHash, sourceInterface: InterfaceId, plaintext: [UInt8])
    case request(destination: DestinationHash, linkId: LinkId, requestId: RequestId, requester: IdentityHash?, pathHash: RequestPathHash, rttMillis: UInt64, data: [UInt8])
    case response(linkId: LinkId, requestId: RequestId, data: [UInt8])
    case responseSegment(linkId: LinkId, requestId: RequestId, segmentIndex: UInt64, totalSegments: UInt64, data: [UInt8])
    case resourceAvailable(linkId: LinkId, hash: ResourceHash, metadata: [UInt8]?, resource: any ResourceStream)
    case resourceSegment(linkId: LinkId, originalHash: ResourceHash, segmentIndex: UInt64, totalSegments: UInt64, metadata: [UInt8]?, data: [UInt8])
    case resourceNeedsDecompression(linkId: LinkId, hash: ResourceHash, stream: [UInt8], uncompressedDataBytes: UInt64)
    case channelMessage(linkId: LinkId, messageType: String, data: [UInt8])
}

public enum DiagnosticEvent: Sendable {
    case announceHeard(destination: DestinationHash, hops: UInt8, sourceInterface: InterfaceId)
    case linkEstablished(linkId: LinkId, rttMillis: UInt64)
    case peerIdentified(linkId: LinkId, identity: IdentityHash)
    case linkClosed(linkId: LinkId, reason: LinkClosedReason)
    case linkInterfaceMismatch(linkId: LinkId, attachedInterface: InterfaceId, arrivedOn: InterfaceId)
    case resourceAssembled(linkId: LinkId, originalHash: ResourceHash, totalSizeBytes: UInt64)
    case resourceFailed(linkId: LinkId, hash: ResourceHash, cause: String)
    case resourceSendProgress(linkId: LinkId, transferredBytes: UInt64, totalBytes: UInt64, physicalTransferredBytes: UInt64, segmentIndex: UInt64, totalSegments: UInt64)
    case selfRatchetRotated(destination: DestinationHash)
    case announceHeldDropped(destination: DestinationHash, sourceInterface: InterfaceId, cause: String)
    case delivered(detail: String)
    case routeExpired(destination: DestinationHash)
    case routeEvicted(destination: DestinationHash)
    case routeInterfaceGone(destination: DestinationHash)
    case routeDropped(destination: DestinationHash)
    case backendDiagnostic(kind: String, detail: String)
    case diagnosticsDropped(count: UInt128)
}

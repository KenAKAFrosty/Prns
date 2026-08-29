import Foundation

public enum HostContract {
    public static let abi: UInt32 = 1
    public static let schemaVersion: UInt32 = 1
    public static let productVersion = "0.3.7"
    public static let destinationHashLength = 16
    public static let identityHashLength = 16
    public static let interfaceIdLength = 8
    public static let linkIdLength = 16
    public static let packetHashLength = 32
    public static let requestIdLength = 16
    public static let requestPathHashLength = 16
    public static let resourceHashLength = 32
    public static let identitySecretLength = 64
    public static let safeIntMin: Int64 = -9007199254740991
    public static let safeIntMax: Int64 = 9007199254740991
    public static let safeUintMax: UInt64 = 9007199254740991
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
    case unsupported = 13
    case permissionDenied = 14
    case unavailable = 15
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
    case suppliedPipe = 13
}

public enum InterfaceKind: UInt32, Sendable {
    case autoLan = 1
    case tcpClient = 2
    case tcpServer = 3
    case udp = 4
    case serial = 5
    case kiss = 6
    case ax25Kiss = 7
    case rNode = 8
    case multiRNode = 9
    case pipe = 10
    case backboneClient = 11
    case backboneServer = 12
    case i2p = 13
    case weave = 14
    case automaticUsb = 15
    case automaticBluetoothLe = 16
    case webSocketClient = 17
    case webSocketServer = 18
    case browserRendezvous = 19
}

public enum InterfaceMode: UInt32, Sendable {
    case full = 1
    case pointToPoint = 2
    case accessPoint = 3
    case roaming = 4
    case boundary = 5
    case gateway = 6
    case `internal` = 7
}

public enum WebSocketFramingSelection: UInt32, Sendable {
    case rawPacket = 1
    case hdlc = 2
    case kiss = 3
    case auto = 4
}

public enum InterfaceHealth: UInt32, Sendable {
    case initializing = 1
    case connected = 2
    case degraded = 3
    case reconnecting = 4
    case failed = 5
    case disconnected = 6
    case disabled = 7
    case unknown = 8
}

public enum DiscoveryScope: UInt32, Sendable {
    case link = 1
    case admin = 2
    case site = 3
    case organization = 4
    case global = 5
}

public enum MulticastAddressType: UInt32, Sendable {
    case temporary = 1
    case permanent = 2
}

public enum SerialDataBits: UInt32, Sendable {
    case five = 5
    case six = 6
    case seven = 7
    case eight = 8
}

public enum SerialParity: UInt32, Sendable {
    case none = 1
    case even = 2
    case odd = 3
}

public enum SerialStopBits: UInt32, Sendable {
    case one = 1
    case two = 2
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

public enum PersistenceConfigKind: UInt32, Sendable {
    case ephemeral = 1
    case directory = 2
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

public enum ResponseTimeoutKind: UInt32, Sendable {
    case linkDefault = 1
    case exact = 2
}

public enum ResourceCompressionKind: UInt32, Sendable {
    case auto = 1
    case never = 2
}

public enum ResourceStrategyKind: UInt32, Sendable {
    case refuse = 1
    case accept = 2
}

public enum RequestPolicy: UInt32, Sendable {
    case allowNone = 1
    case allowAll = 2
    case allowList = 3
}

public enum CommandOutcomeKind: UInt32, Sendable {
    case announced = 1
    case packetDelivered = 2
    case linkCloseQueued = 3
    case interfaceAttached = 4
    case interfaceDetached = 5
    case linkEstablished = 6
    case pathDiscovered = 7
    case identified = 8
    case responseReceived = 9
    case responseSent = 10
    case resourceSent = 11
    case resourceStrategySet = 12
    case requesterAllowed = 13
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
    case unsupportedByBackend = 15
    case unknownLink = 16
    case linkNotActive = 17
    case entropyUnavailable = 18
    case notLinkInitiator = 19
    case identityNotHeld = 20
    case unknownRequestHandler = 21
    case requestPolicyNotAllowList = 22
    case requestAllowListFull = 23
    case linkBusy = 24
    case resourceTableFull = 25
    case resourceMetadataTooLarge = 26
    case resourceRejectedByPeer = 27
    case resourceSequencingFailed = 28
    case resourcePredecessorFailed = 29
    case channelWindowFull = 30
    case channelUntrackable = 31
    case invalidChannelMessageType = 32
    case invalidConfiguration = 33
    case resourceUploadCancelled = 34
    case resourceEarlyEof = 35
    case resourceLengthOverrun = 36
    case permissionDenied = 37
    case deviceUnavailable = 38
    case connectFailed = 39
    case backendFailed = 40
    case responseTooLarge = 41
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
    case linkDelivery = 108
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
    case persistenceRestored = 217
    case persistenceFlushed = 218
    case persistenceFlushFailed = 219
}

public enum PersistenceFlushCause: UInt32, Sendable {
    case startup = 1
    case interval = 2
    case routeChange = 3
    case ratchetRotation = 4
    case shutdown = 5
}

public enum PersistenceFlushTarget: UInt32, Sendable {
    case routingState = 1
    case ratchets = 2
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
    case routes = 32
    case destinationIdentities = 33
    case tunnels = 34
    case ratchets = 35
    case refused = 36
    case dropped = 37
    case persistenceCause = 38
    case persistenceTarget = 39
    case appData = 40
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

public struct RequestHandlerConfig: Hashable, Sendable {
    public let path: String
    public let policy: RequestPolicy

    public init(path: String, policy: RequestPolicy) {
        self.path = path
        self.policy = policy
    }
}

public struct SerialLineConfig: Hashable, Sendable {
    public let baud: UInt32
    public let dataBits: SerialDataBits
    public let parity: SerialParity
    public let stopBits: SerialStopBits

    public init(baud: UInt32, dataBits: SerialDataBits, parity: SerialParity, stopBits: SerialStopBits) {
        self.baud = baud
        self.dataBits = dataBits
        self.parity = parity
        self.stopBits = stopBits
    }
}

public struct RNodeRadioConfig: Hashable, Sendable {
    public let frequencyHz: UInt64
    public let bandwidthHz: UInt32
    public let txPowerDbm: Int16
    public let spreadingFactor: UInt8
    public let codingRate: UInt8

    public init(frequencyHz: UInt64, bandwidthHz: UInt32, txPowerDbm: Int16, spreadingFactor: UInt8, codingRate: UInt8) {
        self.frequencyHz = frequencyHz
        self.bandwidthHz = bandwidthHz
        self.txPowerDbm = txPowerDbm
        self.spreadingFactor = spreadingFactor
        self.codingRate = codingRate
    }
}

public struct MultiRNodeMemberConfig: Hashable, Sendable {
    public let name: String
    public let virtualPort: UInt8
    public let radio: RNodeRadioConfig
    public let flowControl: Bool
    public let outgoing: Bool

    public init(name: String, virtualPort: UInt8, radio: RNodeRadioConfig, flowControl: Bool, outgoing: Bool) {
        self.name = name
        self.virtualPort = virtualPort
        self.radio = radio
        self.flowControl = flowControl
        self.outgoing = outgoing
    }
}

public struct InterfaceRoutingPolicy: Hashable, Sendable {
    public let mode: InterfaceMode?
    public let gravity: Int64?
    public let recursivePathRequests: Bool?
    public let announcesFromInternal: Bool?
    public let announcesToInternal: Bool?

    public init(mode: InterfaceMode?, gravity: Int64?, recursivePathRequests: Bool?, announcesFromInternal: Bool?, announcesToInternal: Bool?) {
        self.mode = mode
        self.gravity = gravity
        self.recursivePathRequests = recursivePathRequests
        self.announcesFromInternal = announcesFromInternal
        self.announcesToInternal = announcesToInternal
    }
}

public struct BackendInfo: Hashable, Sendable {
    public let backend: BackendKind
    public let capabilities: [Capability]
    public let interfaceKinds: [InterfaceKind]

    public init(backend: BackendKind, capabilities: [Capability], interfaceKinds: [InterfaceKind]) {
        self.backend = backend
        self.capabilities = capabilities
        self.interfaceKinds = interfaceKinds
    }
}

public struct InterfaceSnapshot: Sendable {
    public let interfaceId: InterfaceId
    public let name: String?
    public let kind: InterfaceKind?
    public let health: InterfaceHealth
    public let failureDetail: String?
    public let rxBytes: UInt64
    public let txBytes: UInt64
    public let rxBps: UInt64?
    public let txBps: UInt64?
    public let routeCount: UInt32
    public let linkCount: UInt32
    public let transportedLinkCount: UInt32

    public init(interfaceId: InterfaceId, name: String?, kind: InterfaceKind?, health: InterfaceHealth, failureDetail: String?, rxBytes: UInt64, txBytes: UInt64, rxBps: UInt64?, txBps: UInt64?, routeCount: UInt32, linkCount: UInt32, transportedLinkCount: UInt32) {
        self.interfaceId = interfaceId
        self.name = name
        self.kind = kind
        self.health = health
        self.failureDetail = failureDetail
        self.rxBytes = rxBytes
        self.txBytes = txBytes
        self.rxBps = rxBps
        self.txBps = txBps
        self.routeCount = routeCount
        self.linkCount = linkCount
        self.transportedLinkCount = transportedLinkCount
    }
}

public struct RouteSnapshot: Sendable {
    public let destination: DestinationHash
    public let hops: UInt8
    public let viaIdentity: IdentityHash?
    public let interfaceId: InterfaceId
    public let learnedAtMillis: UInt64
    public let lastRouteActivityAtMillis: UInt64
    public let expiresAtMillis: UInt64

    public init(destination: DestinationHash, hops: UInt8, viaIdentity: IdentityHash?, interfaceId: InterfaceId, learnedAtMillis: UInt64, lastRouteActivityAtMillis: UInt64, expiresAtMillis: UInt64) {
        self.destination = destination
        self.hops = hops
        self.viaIdentity = viaIdentity
        self.interfaceId = interfaceId
        self.learnedAtMillis = learnedAtMillis
        self.lastRouteActivityAtMillis = lastRouteActivityAtMillis
        self.expiresAtMillis = expiresAtMillis
    }
}

public struct DestinationIdentitySnapshot: Sendable {
    public let destination: DestinationHash
    public let identity: IdentityHash

    public init(destination: DestinationHash, identity: IdentityHash) {
        self.destination = destination
        self.identity = identity
    }
}

public struct RuntimeHealthSnapshot: Sendable {
    public let running: Bool
    public let uptimeMillis: UInt64
    public let interfaceCount: UInt32
    public let onlineInterfaceCount: UInt32
    public let routeCount: UInt32
    public let linkCount: UInt32
    public let transportedLinkCount: UInt32
    public let rxBytes: UInt64
    public let txBytes: UInt64
    public let rxBps: UInt64
    public let txBps: UInt64

    public init(running: Bool, uptimeMillis: UInt64, interfaceCount: UInt32, onlineInterfaceCount: UInt32, routeCount: UInt32, linkCount: UInt32, transportedLinkCount: UInt32, rxBytes: UInt64, txBytes: UInt64, rxBps: UInt64, txBps: UInt64) {
        self.running = running
        self.uptimeMillis = uptimeMillis
        self.interfaceCount = interfaceCount
        self.onlineInterfaceCount = onlineInterfaceCount
        self.routeCount = routeCount
        self.linkCount = linkCount
        self.transportedLinkCount = transportedLinkCount
        self.rxBytes = rxBytes
        self.txBytes = txBytes
        self.rxBps = rxBps
        self.txBps = txBps
    }
}

public struct PersistenceSnapshot: Sendable {
    public let persistent: Bool
    public let restored: Bool
    public let lastFlushCause: PersistenceFlushCause?
    public let lastFailureDetail: String?

    public init(persistent: Bool, restored: Bool, lastFlushCause: PersistenceFlushCause?, lastFailureDetail: String?) {
        self.persistent = persistent
        self.restored = restored
        self.lastFlushCause = lastFlushCause
        self.lastFailureDetail = lastFailureDetail
    }
}

public struct HostSnapshot: Sendable {
    public let revision: UInt64
    public let backend: BackendInfo
    public let interfaces: [InterfaceSnapshot]
    public let routes: [RouteSnapshot]
    public let activeLinkCount: UInt32
    public let destinationIdentities: [DestinationIdentitySnapshot]
    public let runtime: RuntimeHealthSnapshot
    public let persistence: PersistenceSnapshot

    public init(revision: UInt64, backend: BackendInfo, interfaces: [InterfaceSnapshot], routes: [RouteSnapshot], activeLinkCount: UInt32, destinationIdentities: [DestinationIdentitySnapshot], runtime: RuntimeHealthSnapshot, persistence: PersistenceSnapshot) {
        self.revision = revision
        self.backend = backend
        self.interfaces = interfaces
        self.routes = routes
        self.activeLinkCount = activeLinkCount
        self.destinationIdentities = destinationIdentities
        self.runtime = runtime
        self.persistence = persistence
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

public enum PersistenceConfig: Sendable {
    case ephemeral
    case directory(path: String)
}

public enum InterfaceConfig: Sendable {
    case autoLan(groupId: String?, discoveryScope: DiscoveryScope?, discoveryPort: UInt16?, dataPort: UInt16?, devices: [String], ignoredDevices: [String], multicastAddressType: MulticastAddressType?)
    case tcpClient(target: String, bitrate: Bitrate)
    case tcpServer(bind: String, bitrate: Bitrate)
    case udp(local: String, peer: String, bitrate: Bitrate)
    case serial(port: String, line: SerialLineConfig)
    case kiss(port: String, line: SerialLineConfig, flowControl: Bool, preambleMillis: UInt32, transmitTailMillis: UInt32, persistence: UInt8, slotTimeMillis: UInt32, stationCallsign: String?, stationIntervalSeconds: UInt64?)
    case ax25Kiss(port: String, line: SerialLineConfig, flowControl: Bool, preambleMillis: UInt32, transmitTailMillis: UInt32, persistence: UInt8, slotTimeMillis: UInt32, callsign: String, ssid: UInt8)
    case rNode(port: String, radio: RNodeRadioConfig, flowControl: Bool, stationCallsign: String?, stationIntervalSeconds: UInt64?, airtimeLimitShortCentiPercent: UInt16?, airtimeLimitLongCentiPercent: UInt16?)
    case multiRNode(port: String, stationCallsign: String?, stationIntervalSeconds: UInt64?, members: [MultiRNodeMemberConfig])
    case pipe(command: [String], respawnDelayMillis: UInt64)
    case backboneClient(target: String, bitrate: Bitrate)
    case backboneServer(bind: String, bitrate: Bitrate)
    case i2p(peers: [String], connectable: Bool)
    case weave(port: String)
    case automaticUsb
    case automaticBluetoothLe
    case webSocketClient(target: String, framing: WebSocketFramingSelection)
    case webSocketServer(bind: String, framing: WebSocketFramingSelection)
    case browserRendezvous(url: String)
}

public enum DestinationIdentityConfig: Sendable {
    case hostIdentity
    case dedicatedIdentity(identity: IdentityConfig)
}

public enum Bitrate: Sendable {
    case auto
    case bitsPerSecond(value: UInt64)
}

public enum ResponseTimeout: Sendable {
    case linkDefault
    case exact(millis: UInt64)
}

public enum ResourceCompression: Sendable {
    case auto
    case never
}

public enum ResourceStrategy: Sendable {
    case refuse
    case accept(maximumUncompressedBytes: UInt64, acceptCompressed: Bool)
}

public enum DestinationConfig: Sendable {
    case plain(name: DestinationName)
    case single(name: DestinationName, identity: DestinationIdentityConfig, announceAppData: [UInt8]?, maximumRequestBytes: UInt64?, requestHandlers: [RequestHandlerConfig])
}

public enum HostCommand: Sendable {
    case announce(destination: DestinationHash, interface: InterfaceId?)
    case sendSinglePacket(destination: DestinationHash, payload: [UInt8])
    case closeLink(linkId: LinkId)
    case attachTcpServer(bind: String, bitrate: Bitrate)
    case attachTcpClient(target: String, bitrate: Bitrate)
    case attachUdp(local: String, peer: String, bitrate: Bitrate)
    case detachInterface(interface: InterfaceId)
    case establishLink(destination: DestinationHash)
    case requestPath(destination: DestinationHash)
    case identify(linkId: LinkId, identity: IdentityHash)
    case sendLinkPacket(linkId: LinkId, payload: [UInt8])
    case request(linkId: LinkId, pathHash: RequestPathHash, payload: [UInt8], timeout: ResponseTimeout, maximumResponseBytes: UInt64?)
    case respond(linkId: LinkId, requestId: RequestId, requestRttMillis: UInt64, payload: [UInt8])
    case sendResource(linkId: LinkId, payload: [UInt8], packedMetadata: [UInt8]?, compression: ResourceCompression)
    case setLinkResourceStrategy(linkId: LinkId, strategy: ResourceStrategy)
    case setDestinationResourceStrategy(destination: DestinationHash, strategy: ResourceStrategy)
    case sendChannelMessage(linkId: LinkId, messageType: UInt16, payload: [UInt8])
    case allowRequester(destination: DestinationHash, pathHash: RequestPathHash, identity: IdentityHash)
    case attachInterface(config: InterfaceConfig, routing: InterfaceRoutingPolicy?)
}

public enum CommandOutcome: Sendable {
    case announced
    case packetDelivered(rttMillis: UInt64, evidence: DeliveryEvidenceKind, packetHash: PacketHash?)
    case linkCloseQueued
    case interfaceAttached(interface: InterfaceId)
    case interfaceDetached(interface: InterfaceId)
    case linkEstablished(linkId: LinkId, rttMillis: UInt64)
    case pathDiscovered(hops: UInt8)
    case identified
    case responseReceived(data: [UInt8], rttMillis: UInt64)
    case responseSent(rttMillis: UInt64)
    case resourceSent
    case resourceStrategySet
    case requesterAllowed
}

public enum CommandFailure: Sendable {
    case nodeStopped
    case busy
    case payloadTooLarge
    case unknownDestination
    case notSingleDestination
    case announceAppDataTooLong
    case unknownInterface
    case noRouteToDestination
    case notDirectlyReachable
    case packetCulled
    case deliveryTimedOut
    case invalidBitrate
    case bindFailed(detail: String)
    case writeFailed(detail: String)
    case unsupportedByBackend
    case unknownLink
    case linkNotActive
    case entropyUnavailable
    case notLinkInitiator
    case identityNotHeld
    case unknownRequestHandler
    case requestPolicyNotAllowList
    case requestAllowListFull
    case linkBusy
    case resourceTableFull
    case resourceMetadataTooLarge
    case resourceRejectedByPeer
    case resourceSequencingFailed
    case resourcePredecessorFailed
    case channelWindowFull
    case channelUntrackable
    case invalidChannelMessageType
    case invalidConfiguration(detail: String)
    case resourceUploadCancelled
    case resourceEarlyEof
    case resourceLengthOverrun
    case permissionDenied(detail: String)
    case deviceUnavailable(detail: String)
    case connectFailed(detail: String)
    case backendFailed(detail: String)
    case responseTooLarge
}

public enum ApplicationEvent: Sendable {
    case singleDelivery(destination: DestinationHash, sourceInterface: InterfaceId, plaintext: [UInt8])
    case request(destination: DestinationHash, linkId: LinkId, requestId: RequestId, requester: IdentityHash?, pathHash: RequestPathHash, rttMillis: UInt64, data: [UInt8])
    case response(linkId: LinkId, requestId: RequestId, data: [UInt8])
    case responseSegment(linkId: LinkId, requestId: RequestId, segmentIndex: UInt64, totalSegments: UInt64, data: [UInt8])
    case resourceAvailable(linkId: LinkId, hash: ResourceHash, metadata: [UInt8]?, resource: any ResourceStream)
    case resourceSegment(linkId: LinkId, originalHash: ResourceHash, segmentIndex: UInt64, totalSegments: UInt64, metadata: [UInt8]?, data: [UInt8])
    case resourceNeedsDecompression(linkId: LinkId, hash: ResourceHash, stream: [UInt8], uncompressedDataBytes: UInt64)
    case channelMessage(linkId: LinkId, messageType: UInt16, data: [UInt8])
    case linkDelivery(linkId: LinkId, sourceInterface: InterfaceId, plaintext: [UInt8])
}

public enum DiagnosticEvent: Sendable {
    case announceHeard(destination: DestinationHash, hops: UInt8, sourceInterface: InterfaceId, appData: [UInt8])
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
    case persistenceRestored(routes: UInt64, destinationIdentities: UInt64, tunnels: UInt64, ratchets: UInt64, refused: UInt64, dropped: UInt64)
    case persistenceFlushed(cause: PersistenceFlushCause, target: PersistenceFlushTarget)
    case persistenceFlushFailed(cause: PersistenceFlushCause, target: PersistenceFlushTarget)
}

let hostOperationNames = [
    "contractInfo",
    "backendInfo",
    "hostCreate",
    "hostRelease",
    "hostLifecycle",
    "hostSnapshot",
    "hostSnapshotRead",
    "hostSnapshotRelease",
    "hostIdentityHash",
    "hostDestinationCount",
    "hostDestinationHash",
    "hostAttachSuppliedPipe",
    "suppliedPipeClaimAttachment",
    "suppliedPipeNextOpenRequest",
    "suppliedPipeRegisterReadiness",
    "suppliedPipeInterruptWait",
    "suppliedPipeRelease",
    "suppliedPipeOpenRequestProvide",
    "suppliedPipeOpenRequestDecline",
    "suppliedPipeOpenRequestRelease",
    "hostBeginResourceUpload",
    "resourceUploadWrite",
    "resourceUploadIsWritable",
    "resourceUploadFinish",
    "resourceUploadAbort",
    "resourceUploadRelease",
    "hostStop",
    "commandWait",
    "commandRegisterReadiness",
    "commandInterruptWait",
    "commandRelease",
    "hostClaimApplicationEvents",
    "hostClaimDiagnostics",
    "eventStreamRegisterReadiness",
    "readinessRegistrationRelease",
    "eventStreamInterruptWait",
    "eventStreamRelease",
    "eventStreamNext",
    "eventRelease",
    "eventKind",
    "eventBytes",
    "eventString",
    "eventU64",
    "eventU128",
    "eventResourceStream",
    "resourceStreamRelease",
    "resourceStreamNext",
    "hostAnnounce",
    "hostSendSinglePacket",
    "hostCloseLink",
    "hostAttachTcpServer",
    "hostAttachTcpClient",
    "hostAttachUdp",
    "hostDetachInterface",
    "hostEstablishLink",
    "hostRequestPath",
    "hostIdentify",
    "hostSendLinkPacket",
    "hostRequest",
    "hostRespond",
    "hostSendResource",
    "hostSetLinkResourceStrategy",
    "hostSetDestinationResourceStrategy",
    "hostSendChannelMessage",
    "hostAllowRequester",
    "hostAttachInterface",
]

struct RawUnit {}
struct RawOwned<Value> { let value: Value }
struct RawBorrowed<Value> { let value: Value }
enum RawCallResult<Value> {
    case success(Value)
    case failure(Status)
}
struct RawCommandResult {}
struct RawContractInfo {}
struct RawEvent {}
struct RawEventStream {}
struct RawHost {}
struct RawHostInspection {}
struct RawHostOptions {}
struct RawIssuedCommand {}
struct RawLifecycle {}
struct RawReadinessCallback {}
struct RawReadinessRegistration {}
struct RawResourceChunk {}
struct RawResourceStream {}
struct RawResourceUpload {}
struct RawSuppliedPipe {}
struct RawSuppliedPipeOpenRequest {}
struct RawOpaquePointer {}

protocol RawHostProtocol {
    func contractInfo() -> RawCallResult<RawContractInfo>
    func backendInfo() -> RawCallResult<BackendInfo>
    func hostCreate(_ options: RawHostOptions) -> RawCallResult<RawOwned<RawHost>>
    func hostRelease(_ host: RawHost) -> RawUnit
    func hostLifecycle(_ host: RawHost) -> RawCallResult<RawLifecycle>
    func hostSnapshot(_ host: RawHost, _ timeoutMillis: UInt32) -> RawCallResult<RawOwned<RawHostInspection>>
    func hostSnapshotRead(_ host_inspection: RawHostInspection) -> RawCallResult<RawBorrowed<HostSnapshot>>
    func hostSnapshotRelease(_ host_inspection: RawHostInspection) -> RawUnit
    func hostIdentityHash(_ host: RawHost) -> RawCallResult<RawBorrowed<[UInt8]>>
    func hostDestinationCount(_ host: RawHost) -> Int
    func hostDestinationHash(_ host: RawHost, _ index: Int) -> RawCallResult<RawBorrowed<[UInt8]>>
    func hostAttachSuppliedPipe(_ host: RawHost, _ name: String, _ respawnDelayMillis: UInt64, _ bitrate: Bitrate) -> RawCallResult<RawOwned<RawSuppliedPipe>>
    func suppliedPipeClaimAttachment(_ supplied_pipe: RawSuppliedPipe) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func suppliedPipeNextOpenRequest(_ supplied_pipe: RawSuppliedPipe, _ timeoutMillis: UInt32) -> RawCallResult<RawOwned<RawSuppliedPipeOpenRequest>>
    func suppliedPipeRegisterReadiness(_ supplied_pipe: RawSuppliedPipe, _ callback: RawReadinessCallback, _ context: RawOpaquePointer) -> RawCallResult<RawOwned<RawReadinessRegistration>>
    func suppliedPipeInterruptWait(_ supplied_pipe: RawSuppliedPipe) -> RawUnit
    func suppliedPipeRelease(_ supplied_pipe: RawSuppliedPipe) -> RawUnit
    func suppliedPipeOpenRequestProvide(_ supplied_pipe_open_request: RawSuppliedPipeOpenRequest, _ descriptor: Int64) -> RawCallResult<Bool>
    func suppliedPipeOpenRequestDecline(_ supplied_pipe_open_request: RawSuppliedPipeOpenRequest) -> RawCallResult<Bool>
    func suppliedPipeOpenRequestRelease(_ supplied_pipe_open_request: RawSuppliedPipeOpenRequest) -> RawUnit
    func hostBeginResourceUpload(_ host: RawHost, _ linkId: LinkId, _ declaredLength: UInt64, _ packedMetadata: [UInt8]?, _ compression: ResourceCompression) -> RawCallResult<RawOwned<RawResourceUpload>>
    func resourceUploadWrite(_ resource_upload: RawResourceUpload, _ chunk: [UInt8]) -> RawCallResult<RawUnit>
    func resourceUploadIsWritable(_ resource_upload: RawResourceUpload) -> RawCallResult<Bool>
    func resourceUploadFinish(_ resource_upload: RawResourceUpload) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func resourceUploadAbort(_ resource_upload: RawResourceUpload) -> RawUnit
    func resourceUploadRelease(_ resource_upload: RawResourceUpload) -> RawUnit
    func hostStop(_ host: RawHost) -> RawCallResult<RawUnit>
    func commandWait(_ issued_command: RawIssuedCommand, _ timeoutMillis: UInt32) -> RawCallResult<RawBorrowed<RawCommandResult>>
    func commandRegisterReadiness(_ issued_command: RawIssuedCommand, _ callback: RawReadinessCallback, _ context: RawOpaquePointer) -> RawCallResult<RawOwned<RawReadinessRegistration>>
    func commandInterruptWait(_ issued_command: RawIssuedCommand) -> RawUnit
    func commandRelease(_ issued_command: RawIssuedCommand) -> RawUnit
    func hostClaimApplicationEvents(_ host: RawHost) -> RawCallResult<RawOwned<RawEventStream>>
    func hostClaimDiagnostics(_ host: RawHost) -> RawCallResult<RawOwned<RawEventStream>>
    func eventStreamRegisterReadiness(_ event_stream: RawEventStream, _ callback: RawReadinessCallback, _ context: RawOpaquePointer) -> RawCallResult<RawOwned<RawReadinessRegistration>>
    func readinessRegistrationRelease(_ readiness_registration: RawReadinessRegistration) -> RawUnit
    func eventStreamInterruptWait(_ event_stream: RawEventStream) -> RawUnit
    func eventStreamRelease(_ event_stream: RawEventStream) -> RawUnit
    func eventStreamNext(_ event_stream: RawEventStream, _ timeoutMillis: UInt32) -> RawCallResult<RawOwned<RawEvent>>
    func eventRelease(_ eventValue: RawEvent) -> RawUnit
    func eventKind(_ eventValue: RawEvent) -> UInt32
    func eventBytes(_ eventValue: RawEvent, _ field: EventField) -> RawCallResult<RawBorrowed<[UInt8]>>
    func eventString(_ eventValue: RawEvent, _ field: EventField) -> RawCallResult<RawBorrowed<String>>
    func eventU64(_ eventValue: RawEvent, _ field: EventField) -> RawCallResult<UInt64>
    func eventU128(_ eventValue: RawEvent, _ field: EventField) -> RawCallResult<UInt128>
    func eventResourceStream(_ eventValue: RawEvent) -> RawCallResult<RawOwned<RawResourceStream>>
    func resourceStreamRelease(_ resource_stream: RawResourceStream) -> RawUnit
    func resourceStreamNext(_ resource_stream: RawResourceStream, _ maximumBytes: Int) -> RawCallResult<RawBorrowed<RawResourceChunk>>
    func hostAnnounce(_ host: RawHost, _ destination: DestinationHash, _ interfaceId: InterfaceId?) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostSendSinglePacket(_ host: RawHost, _ destination: DestinationHash, _ payload: [UInt8]) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostCloseLink(_ host: RawHost, _ linkId: LinkId) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostAttachTcpServer(_ host: RawHost, _ bind: String, _ bitrate: Bitrate) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostAttachTcpClient(_ host: RawHost, _ target: String, _ bitrate: Bitrate) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostAttachUdp(_ host: RawHost, _ local: String, _ peer: String, _ bitrate: Bitrate) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostDetachInterface(_ host: RawHost, _ interfaceId: InterfaceId) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostEstablishLink(_ host: RawHost, _ destination: DestinationHash) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostRequestPath(_ host: RawHost, _ destination: DestinationHash) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostIdentify(_ host: RawHost, _ linkId: LinkId, _ identity: IdentityHash) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostSendLinkPacket(_ host: RawHost, _ linkId: LinkId, _ payload: [UInt8]) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostRequest(_ host: RawHost, _ linkId: LinkId, _ pathHash: RequestPathHash, _ payload: [UInt8], _ timeout: ResponseTimeout, _ maximumResponseBytes: UInt64?) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostRespond(_ host: RawHost, _ linkId: LinkId, _ requestId: RequestId, _ requestRttMillis: UInt64, _ payload: [UInt8]) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostSendResource(_ host: RawHost, _ linkId: LinkId, _ payload: [UInt8], _ packedMetadata: [UInt8]?, _ compression: ResourceCompression) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostSetLinkResourceStrategy(_ host: RawHost, _ linkId: LinkId, _ strategy: ResourceStrategy) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostSetDestinationResourceStrategy(_ host: RawHost, _ destination: DestinationHash, _ strategy: ResourceStrategy) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostSendChannelMessage(_ host: RawHost, _ linkId: LinkId, _ messageType: UInt16, _ payload: [UInt8]) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostAllowRequester(_ host: RawHost, _ destination: DestinationHash, _ pathHash: RequestPathHash, _ identity: IdentityHash) -> RawCallResult<RawOwned<RawIssuedCommand>>
    func hostAttachInterface(_ host: RawHost, _ config: InterfaceConfig, _ routing: InterfaceRoutingPolicy?) -> RawCallResult<RawOwned<RawIssuedCommand>>
}

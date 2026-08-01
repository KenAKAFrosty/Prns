import Foundation
@testable import PersonalRns
import Testing
#if canImport(Glibc)
import Glibc
#else
import Darwin
#endif

@Test
func everySharedInterfaceFixtureMarshals() throws {
    let fixture = try loadInterfaceFixture()
    let line = SerialLineConfig(
        baud: 115_200,
        dataBits: .eight,
        parity: .none,
        stopBits: .one
    )
    let ax25Line = SerialLineConfig(
        baud: 9_600,
        dataBits: .eight,
        parity: .none,
        stopBits: .one
    )
    let radio = RNodeRadioConfig(
        frequencyHz: 915_000_000,
        bandwidthHz: 125_000,
        txPowerDbm: 14,
        spreadingFactor: 8,
        codingRate: 5
    )
    let configs: [InterfaceConfig] = [
        .autoLan(
            groupId: "sdk-fixture",
            discoveryScope: .organization,
            discoveryPort: 29_710,
            dataPort: 42_444,
            devices: ["eth0"],
            ignoredDevices: ["lo"],
            multicastAddressType: .permanent
        ),
        .tcpClient(target: "127.0.0.1:4242", bitrate: .bitsPerSecond(value: 1_000_000)),
        .tcpServer(bind: "127.0.0.1:4242", bitrate: .auto),
        .udp(
            local: "127.0.0.1:4242",
            peer: "127.0.0.1:4243",
            bitrate: .bitsPerSecond(value: 2_000_000)
        ),
        .serial(port: "/dev/ttyUSB0", line: line),
        .kiss(
            port: "/dev/ttyUSB1",
            line: line,
            flowControl: true,
            preambleMillis: 150,
            transmitTailMillis: 50,
            persistence: 64,
            slotTimeMillis: 20,
            stationCallsign: "PRNS",
            stationIntervalSeconds: 300
        ),
        .ax25Kiss(
            port: "/dev/ttyUSB2",
            line: ax25Line,
            flowControl: false,
            preambleMillis: 100,
            transmitTailMillis: 25,
            persistence: 32,
            slotTimeMillis: 10,
            callsign: "PRNS",
            ssid: 1
        ),
        .rNode(
            port: "/dev/ttyACM0",
            radio: radio,
            flowControl: true,
            stationCallsign: "PRNS",
            stationIntervalSeconds: 300,
            airtimeLimitShortCentiPercent: 1_000,
            airtimeLimitLongCentiPercent: 500
        ),
        .multiRNode(
            port: "/dev/ttyACM1",
            stationCallsign: "PRNS",
            stationIntervalSeconds: 300,
            members: [
                MultiRNodeMemberConfig(
                    name: "primary",
                    virtualPort: 1,
                    radio: radio,
                    flowControl: true,
                    outgoing: true
                )
            ]
        ),
        .pipe(command: ["fixture-command", "--fixture"], respawnDelayMillis: 1_000),
        .backboneClient(target: "127.0.0.1:4244", bitrate: .auto),
        .backboneServer(bind: "127.0.0.1:4245", bitrate: .bitsPerSecond(value: 4_000_000)),
        .i2p(peers: ["fixture.b32.i2p"], connectable: true),
        .weave(port: "/dev/ttyWEAVE0"),
        .automaticUsb,
        .automaticBluetoothLe,
        .webSocketClient(target: "ws://fixture.invalid/client"),
        .webSocketServer(bind: "127.0.0.1:4246"),
        .browserRendezvous(url: "ws://fixture.invalid/rendezvous"),
    ]
    #expect(fixture.schemaVersion == HostContract.schemaVersion)
    #expect(fixture.interfaces.count == configs.count)
    #expect(fixture.interfaces.map(\.kind) == [
        "AutoLan",
        "TcpClient",
        "TcpServer",
        "Udp",
        "Serial",
        "Kiss",
        "Ax25Kiss",
        "RNode",
        "MultiRNode",
        "Pipe",
        "BackboneClient",
        "BackboneServer",
        "I2p",
        "Weave",
        "AutomaticUsb",
        "AutomaticBluetoothLe",
        "WebSocketClient",
        "WebSocketServer",
        "BrowserRendezvous",
    ])
    let arena = NativeArena()
    let kinds = try configs.map { try nativeInterfaceConfig($0, arena: arena).kind }
    #expect(kinds == (1...UInt32(configs.count)).map { $0 })
}

@Test
func nativeHostContract() async throws {
    let host = try Host(
        options: .ephemeralEndpoint(requiredCapabilities: [.tcpClient])
    )
    defer { host.close() }

    #expect(host.identityHash != (try IdentityHash([UInt8](repeating: 0, count: 16))))
    #expect(try host.backendInfo.backend == .native)
    #expect(try host.backendInfo.interfaceKinds.contains(.tcpClient))
    let initialSnapshot = try host.snapshot()
    #expect(initialSnapshot.runtime.running)
    #expect(initialSnapshot.runtime.interfaceCount == 0)

    let firstClaim = try host.claimApplicationEvents()
    guard case .claimed(let events) = firstClaim else {
        Issue.record("first application stream claim was rejected")
        return
    }
    defer { events.close() }

    let secondClaim = try host.claimApplicationEvents()
    guard case .alreadyClaimed = secondClaim else {
        Issue.record("second application stream claim was accepted")
        return
    }

    let waiting = Task {
        var iterator = events.makeAsyncIterator()
        return try await iterator.next()
    }
    waiting.cancel()
    do {
        _ = try await waiting.value
        Issue.record("cancelled event wait completed successfully")
    } catch is CancellationError {
    }

    let attached = try await host.attachInterface(
        config: .tcpClient(target: "127.0.0.1:9", bitrate: .auto)
    )
    guard case .succeeded(.interfaceAttached(let interface)) = attached else {
        Issue.record("attach command did not return an interface")
        return
    }
    let attachedSnapshot = try host.snapshot()
    #expect(attachedSnapshot.runtime.interfaceCount == 1)
    #expect(attachedSnapshot.interfaces.first?.interfaceId == interface)
    let resource = try await host.sendResource(
        linkId: try LinkId([UInt8](repeating: 0, count: 16)),
        payload: Array("bounded upload".utf8),
        compression: .never
    )
    guard case .failed(.unknownLink) = resource else {
        Issue.record("bounded resource upload returned the wrong settlement")
        return
    }

    let detached = try await host.detachInterface(interface: interface)
    guard case .succeeded(.interfaceDetached) = detached else {
        Issue.record("detach command did not settle successfully")
        return
    }
}

@Test
func persistentTwoNodeJourney() async throws {
    let fixture = try loadJourneyFixture()
    #expect(fixture.schemaVersion == HostContract.schemaVersion)
    #expect(fixture.resource.compression == "Never")
    let port = try reserveLoopbackPort()
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("prns-swift-journey-\(UUID().uuidString)")
    try FileManager.default.createDirectory(
        at: root,
        withIntermediateDirectories: true
    )
    defer { _ = try? FileManager.default.removeItem(at: root) }

    let destination = DestinationConfig.single(
        name: DestinationName(
            appName: fixture.destination.appName,
            aspects: fixture.destination.aspects
        ),
        identity: .hostIdentity,
        announceAppData: try decodeHex(fixture.destination.announceAppDataHex),
        requestHandlers: [
            RequestHandlerConfig(path: fixture.request.path, policy: .allowAll)
        ]
    )
    let serverOptions = try HostOptions.persistentEndpoint(
        root: root.appendingPathComponent("server").path,
        destinations: [destination],
        requiredCapabilities: [.tcpServer]
    )
    let clientOptions = try HostOptions.persistentEndpoint(
        root: root.appendingPathComponent("client").path,
        requiredCapabilities: [.tcpClient]
    )

    let server = try Host(options: serverOptions)
    defer { server.close() }
    let client = try Host(options: clientOptions)
    defer { client.close() }
    let serverIdentity = server.identityHash
    let clientIdentity = client.identityHash
    let destinationHash = try #require(server.destinationHashes.first)
    guard case .claimed(let events) = try server.claimApplicationEvents() else {
        Issue.record("server application stream claim was rejected")
        return
    }
    var eventIterator = events.makeAsyncIterator()

    guard case .interfaceAttached = try await successfulOutcome(
        server,
        .attachInterface(
            config: .tcpServer(bind: "127.0.0.1:\(port)", bitrate: .auto)
        )
    ) else {
        Issue.record("server interface attachment returned the wrong outcome")
        return
    }
    guard case .interfaceAttached = try await successfulOutcome(
        client,
        .attachInterface(
            config: .tcpClient(target: "127.0.0.1:\(port)", bitrate: .auto)
        )
    ) else {
        Issue.record("client interface attachment returned the wrong outcome")
        return
    }

    var routed = false
    for _ in 0..<50 where !routed {
        routed = try client.snapshot().routes.contains {
            $0.destination == destinationHash
        }
        if !routed {
            guard case .announced = try await successfulOutcome(
                server,
                .announce(destination: destinationHash, interface: nil)
            ) else {
                Issue.record("announce returned the wrong outcome")
                return
            }
            try await ContinuousClock().sleep(for: .milliseconds(50))
        }
    }
    #expect(routed)

    let linkOutcome = try await successfulOutcome(
        client,
        .establishLink(destination: destinationHash)
    )
    guard case .linkEstablished(let linkId, _) = linkOutcome else {
        Issue.record("link establishment returned the wrong outcome")
        return
    }
    let requestPayload = try decodeHex(fixture.request.payloadHex)
    let responsePayload = try decodeHex(fixture.request.responseHex)
    let requestCommand = try client.execute(
        .request(
            linkId: linkId,
            pathHash: try RequestPathHash(decodeHex(fixture.request.pathHashHex)),
            payload: requestPayload,
            timeout: .exact(millis: fixture.request.timeoutMillis)
        )
    )
    defer { requestCommand.close() }
    let requestResult = Task { try await requestCommand.value() }
    let request = try await nextRequest(&eventIterator)
    #expect(request.data == requestPayload)
    guard case .responseSent = try await successfulOutcome(
        server,
        .respond(
            linkId: request.linkId,
            requestId: request.requestId,
            requestRttMillis: request.rttMillis,
            payload: responsePayload
        )
    ) else {
        Issue.record("response returned the wrong outcome")
        return
    }
    guard case .succeeded(.responseReceived(let response, _)) = try await requestResult.value else {
        Issue.record("request returned the wrong settlement")
        return
    }
    #expect(response == responsePayload)

    guard case .resourceStrategySet = try await successfulOutcome(
        server,
        .setLinkResourceStrategy(
            linkId: request.linkId,
            strategy: .accept(
                maximumUncompressedBytes: fixture.resource.maximumUncompressedBytes,
                acceptCompressed: fixture.resource.acceptCompressed
            )
        )
    ) else {
        Issue.record("resource strategy returned the wrong outcome")
        return
    }
    let chunks = try fixture.resource.chunksHex.map(decodeHex)
    let payload = chunks.flatMap { $0 }
    let metadata = try decodeHex(fixture.resource.metadataHex)
    let upload = try client.beginResourceUpload(
        linkId: linkId,
        declaredLength: UInt64(payload.count),
        packedMetadata: metadata,
        compression: .never
    )
    defer { upload.close() }
    for chunk in chunks {
        try await upload.write(chunk)
    }
    guard case .succeeded(.resourceSent) = try await upload.finish() else {
        Issue.record("resource upload returned the wrong settlement")
        return
    }
    let resource = try await nextResource(&eventIterator)
    #expect(resource.metadata == metadata)
    var received: [UInt8] = []
    for try await chunk in resource.stream {
        guard let bytes = chunk as? [UInt8] else {
            throw JourneyFailure.invalidResourceChunk
        }
        received.append(contentsOf: bytes)
    }
    resource.stream.close()
    #expect(received == payload)

    events.close()
    try client.stop()
    try server.stop()
    client.close()
    server.close()

    let restoredServer = try Host(options: serverOptions)
    defer { restoredServer.close() }
    let restoredClient = try Host(options: clientOptions)
    defer { restoredClient.close() }
    #expect(restoredServer.identityHash == serverIdentity)
    #expect(restoredClient.identityHash == clientIdentity)
    #expect(restoredServer.destinationHashes.first == destinationHash)
    let restoredServerSnapshot = try restoredServer.snapshot()
    let restoredClientSnapshot = try restoredClient.snapshot()
    #expect(restoredServerSnapshot.persistence.restored)
    #expect(restoredClientSnapshot.persistence.restored)
    #expect(restoredClientSnapshot.routes.contains {
        $0.destination == destinationHash
    })
}

private struct JourneyFixture: Decodable {
    let schemaVersion: UInt32
    let destination: JourneyDestination
    let request: JourneyRequest
    let resource: JourneyResource
}

private struct InterfaceFixture: Decodable {
    let schemaVersion: UInt32
    let interfaces: [InterfaceFixtureCase]
}

private struct InterfaceFixtureCase: Decodable {
    let kind: String
}

private struct JourneyDestination: Decodable {
    let appName: String
    let aspects: [String]
    let announceAppDataHex: String
}

private struct JourneyRequest: Decodable {
    let path: String
    let pathHashHex: String
    let payloadHex: String
    let responseHex: String
    let timeoutMillis: UInt64
}

private struct JourneyResource: Decodable {
    let chunksHex: [String]
    let metadataHex: String
    let maximumUncompressedBytes: UInt64
    let acceptCompressed: Bool
    let compression: String
}

private struct ReceivedRequest {
    let linkId: LinkId
    let requestId: RequestId
    let rttMillis: UInt64
    let data: [UInt8]
}

private struct ReceivedResource {
    let metadata: [UInt8]?
    let stream: any ResourceStream
}

private enum JourneyFailure: Error {
    case commandFailed(String)
    case eventStreamEnded
    case invalidHex
    case invalidResourceChunk
    case socket(Int32)
}

private func successfulOutcome(
    _ host: PersonalRns.Host,
    _ command: HostCommand
) async throws -> CommandOutcome {
    let pending = try host.execute(command)
    defer { pending.close() }
    switch try await pending.value() {
    case .succeeded(let outcome):
        return outcome
    case .failed(let failure):
        throw JourneyFailure.commandFailed(String(describing: failure))
    }
}

private func nextRequest(
    _ iterator: inout EventSequence<ApplicationEvent>.AsyncIterator
) async throws -> ReceivedRequest {
    while let event = try await iterator.next() {
        if case .request(
            _,
            let linkId,
            let requestId,
            _,
            _,
            let rttMillis,
            let data
        ) = event {
            return ReceivedRequest(
                linkId: linkId,
                requestId: requestId,
                rttMillis: rttMillis,
                data: data
            )
        }
    }
    throw JourneyFailure.eventStreamEnded
}

private func nextResource(
    _ iterator: inout EventSequence<ApplicationEvent>.AsyncIterator
) async throws -> ReceivedResource {
    while let event = try await iterator.next() {
        if case .resourceAvailable(_, _, let metadata, let resource) = event {
            return ReceivedResource(metadata: metadata, stream: resource)
        }
    }
    throw JourneyFailure.eventStreamEnded
}

private func loadJourneyFixture() throws -> JourneyFixture {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<5 {
        url.deleteLastPathComponent()
    }
    return try JSONDecoder().decode(
        JourneyFixture.self,
        from: Data(contentsOf: url.appendingPathComponent("conformance/persistent-two-node-v1.json"))
    )
}

private func loadInterfaceFixture() throws -> InterfaceFixture {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<5 {
        url.deleteLastPathComponent()
    }
    return try JSONDecoder().decode(
        InterfaceFixture.self,
        from: Data(contentsOf: url.appendingPathComponent("conformance/interface-configs-v1.json"))
    )
}

private func decodeHex(_ value: String) throws -> [UInt8] {
    guard value.count.isMultiple(of: 2) else {
        throw JourneyFailure.invalidHex
    }
    var bytes: [UInt8] = []
    bytes.reserveCapacity(value.count / 2)
    var index = value.startIndex
    while index < value.endIndex {
        let end = value.index(index, offsetBy: 2)
        guard let byte = UInt8(value[index..<end], radix: 16) else {
            throw JourneyFailure.invalidHex
        }
        bytes.append(byte)
        index = end
    }
    return bytes
}

private func reserveLoopbackPort() throws -> UInt16 {
    let descriptor = socket(AF_INET, Int32(SOCK_STREAM.rawValue), 0)
    guard descriptor >= 0 else {
        throw JourneyFailure.socket(errno)
    }
#if canImport(Glibc)
    defer { _ = Glibc.close(descriptor) }
#else
    defer { _ = Darwin.close(descriptor) }
#endif
    var address = sockaddr_in()
#if !canImport(Glibc)
    address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
#endif
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = 0
    address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
    let bindResult = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
#if canImport(Glibc)
            Glibc.bind(
                descriptor,
                $0,
                socklen_t(MemoryLayout<sockaddr_in>.size)
            )
#else
            Darwin.bind(
                descriptor,
                $0,
                socklen_t(MemoryLayout<sockaddr_in>.size)
            )
#endif
        }
    }
    guard bindResult == 0 else {
        throw JourneyFailure.socket(errno)
    }
    var length = socklen_t(MemoryLayout<sockaddr_in>.size)
    let nameResult = withUnsafeMutablePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            getsockname(descriptor, $0, &length)
        }
    }
    guard nameResult == 0 else {
        throw JourneyFailure.socket(errno)
    }
    return UInt16(bigEndian: address.sin_port)
}

import PersonalRns
import Testing

@Test
func nativeHostContract() async throws {
    let host = try Host(
        options: .ephemeralEndpoint(requiredCapabilities: [.tcpClient])
    )
    defer { host.close() }

    #expect(host.identityHash != (try IdentityHash([UInt8](repeating: 0, count: 16))))

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

    let attach = try host.execute(
        .attachTcpClient(target: "127.0.0.1:9", bitrate: .auto)
    )
    defer { attach.close() }
    let attached = try await attach.value()
    guard case .succeeded(.interfaceAttached(let interface)) = attached else {
        Issue.record("attach command did not return an interface")
        return
    }

    let detach = try host.execute(.detachInterface(interface: interface))
    defer { detach.close() }
    let detached = try await detach.value()
    guard case .succeeded(.interfaceDetached) = detached else {
        Issue.record("detach command did not settle successfully")
        return
    }
}

// Shared actual generated-client journey. Both owning images execute these assertions.
// The consumer supplies Foundation and its selected generated module import.
private enum ConnectionConformanceFailure: Error { case failed(String) }
private func connectionRequire(_ value: Bool, _ message: String) throws {
    if !value { throw ConnectionConformanceFailure.failed(message) }
}
private func connectionHost() -> HostConfig {
    HostConfig(identity: .generateEphemeral, persistence: .ephemeral, role: .endpoint,
        destinations: [], requiredCapabilities: [], limits: PrnsLimits(
            pendingCommands: 64, applicationEvents: 64, retainedEventBytes: 1_048_576, diagnostics: 32))
}

private func connectionProfileOptions() -> ConnectionOptions {
    ConnectionOptions(limits: ConnectionLimits(profiles: 8, encodedBytes: 32768),
        queueCapacity: 16, cacheBytes: 1 << 20)
}
private func connectionProfile(_ id: Data, _ label: String = "Relay") throws -> ConnectionProfile {
    ConnectionProfile(id: id, label: label, enabled: true,
        config: try connectionTcpClientConfigFromTarget(target: "127.0.0.1:9", bitrate: .auto), routing: nil)
}
private func connectionApplied(_ binding: ConnectionBindingClient, _ revision: UInt64) async throws -> ConnectionBindingSnapshot {
    for _ in 0..<500 {
        let value = try await binding.snapshot()
        if value.appliedRevision == revision { return value }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    throw ConnectionConformanceFailure.failed("connection reconciliation did not settle")
}
private func connectionReject(_ expected: ConnectionInputError, _ body: () throws -> Void) throws {
    do { try body(); throw ConnectionConformanceFailure.failed("invalid connection input accepted") }
    catch ConnectionError.Invalid(let reason) { try connectionRequire(reason == expected, "wrong checked input error: \(reason)") }
}

// The optional owner is returned so each consumer can explicitly drop the sole
// foreign owner wrapper before awaiting its retained shutdown observer.
private func prepareConnectionsConformance(path: String) async throws
    -> (ConnectionProfiles?, ConnectionSnapshot, UInt64) {
    let files = FileManager.default
    let id = try newConnectionProfileId()
    try connectionRequire(id.count == 16 && id.contains(where: { $0 != 0 }), "generated checked profile ID")
    for badID in [Data([1]), Data(repeating: 0, count: 16)] {
        try connectionReject(.invalidId) { _ = try ConnectionProfiles.createNew(path: path, profiles: [connectionProfile(badID)], options: connectionProfileOptions()) }
    }
    for queue: UInt32 in [0, UInt32.max] {
        var invalid = connectionProfileOptions(); invalid.queueCapacity = queue
        try connectionReject(.invalidLimits) { _ = try ConnectionProfiles.createNew(path: path, profiles: [], options: invalid) }
    }
    var invalid = connectionProfileOptions(); invalid.cacheBytes = UInt64.max
    try connectionReject(.invalidLimits) { _ = try ConnectionProfiles.createNew(path: path, profiles: [], options: invalid) }
    var badConfig = try connectionProfile(id)
    badConfig.config = .tcpClient(target: "https://invalid", bitrate: .auto)
    try connectionReject(.invalidTarget) { _ = try ConnectionProfiles.createNew(path: path, profiles: [badConfig], options: connectionProfileOptions()) }
    badConfig.config = .tcpClient(target: "127.0.0.1:9", bitrate: .bitsPerSecond(value: UInt64.max))
    try connectionReject(.invalidConfig) { _ = try ConnectionProfiles.createNew(path: path, profiles: [badConfig], options: connectionProfileOptions()) }
    badConfig = try connectionProfile(id)
    badConfig.routing = InterfaceRoutingPolicy(mode: nil, gravity: Int64.max,
        recursivePathRequests: nil, announcesFromInternal: nil, announcesToInternal: nil)
    try connectionReject(.invalidConfig) { _ = try ConnectionProfiles.createNew(path: path, profiles: [badConfig], options: connectionProfileOptions()) }
    try connectionRequire(!files.fileExists(atPath: path), "rejected foreign input caused file effects")
    do {
        _ = try ConnectionProfiles.openExisting(path: path, options: connectionProfileOptions())
        throw ConnectionConformanceFailure.failed("open created a missing store")
    } catch ConnectionError.File { }
    try connectionRequire(!files.fileExists(atPath: path), "strict open created a store")

    let profiles: ConnectionProfiles? = try ConnectionProfiles.createNew(path: path, profiles: [], options: connectionProfileOptions())
    let settings = profiles!.client()
    let empty = try await settings.snapshot()
    let saved = try await settings.replace(expected: empty.guard, profiles: [connectionProfile(id)])
    try connectionRequire(saved.guard.revision == empty.guard.revision + 1, "durable stopped edit")
    let unchanged = try await settings.replace(expected: saved.guard, profiles: saved.profiles)
    try connectionRequire(unchanged.guard == saved.guard, "no-op changed revision")
    do {
        _ = try await settings.replace(expected: empty.guard, profiles: [])
        throw ConnectionConformanceFailure.failed("stale CAS accepted")
    } catch ConnectionError.Conflict(let current) { try connectionRequire(current == saved, "conflict lost exact current snapshot") }
    do {
        _ = try await settings.replace(expected: ConnectionGuard(incarnation: Data([1]), revision: 0), profiles: [])
        throw ConnectionConformanceFailure.failed("malformed CAS identifier accepted")
    } catch ConnectionError.Invalid(let reason) { try connectionRequire(reason == .invalidId, "CAS identifier error") }

    // This path needs only a real independent host and a profile owner.
    let session = try await openHost(config: connectionHost())
    var binding: ConnectionHostBinding? = try await profiles!.bindHost(host: session)
    let borrowed = binding!.client()
    let first = try await connectionApplied(borrowed, saved.guard.revision)
    try connectionRequire(first.attachments.count == 1 && first.attachments[0].state == .attached,
        "saved TCP profile was not attached")
    try connectionRequire(try await session.client().snapshot().interfaces.count == 1, "host interface missing")
    binding!.fence()
    try connectionRequire(borrowed.status().phase == .fenced, "binding fence not synchronous")
    try connectionRequire(try await session.client().snapshot().interfaces.count == 1, "fence retired before domain cleanup")
    do {
        try await borrowed.retry(expected: saved.guard, profile: id)
        throw ConnectionConformanceFailure.failed("fenced retry accepted")
    } catch ConnectionError.Closed { }
    let bindingClose = binding!.shutdown()
    binding = nil
    try await bindingClose.wait()
    try await bindingClose.wait() // observation alone cannot trigger another cleanup
    try connectionRequire(borrowed.status().phase == .closed, "binding cleanup not retained")
    try connectionRequire(try await session.client().snapshot().interfaces.isEmpty, "binding close left claimed resources")
    try await session.stop()
    let renamed = try await settings.replace(expected: saved.guard, profiles: [connectionProfile(id, "Stopped edit")])
    try connectionRequire(renamed.profiles[0].label == "Stopped edit", "stopped owner lost editing")

    return (profiles, renamed, first.generation)
}

private func finishConnectionsConformance(
    profiles: inout ConnectionProfiles?, path: String, expected: ConnectionSnapshot
) async throws {
    let settings = profiles!.client()
    let storeClose = profiles!.shutdown(); profiles = nil
    try await storeClose.wait()
    do { _ = try await settings.snapshot(); throw ConnectionConformanceFailure.failed("borrowed profile client kept store active") }
    catch ConnectionError.Closed { }
    profiles = try ConnectionProfiles.openExisting(path: path, options: connectionProfileOptions())
    let persisted = try await profiles!.client().snapshot()
    try connectionRequire(persisted == expected, "strict reopen changed saved intent")
    let lastClose = profiles!.shutdown(); profiles = nil
    try await lastClose.wait()
}

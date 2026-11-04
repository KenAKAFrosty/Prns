// Runs with swift_support.swift and the shared host-owned connections journey.
@main
struct ConnectionsConformance {
    static func main() async throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-connections-swift-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        defer { try? files.removeItem(at: root) }
        let path = root.appendingPathComponent("profiles.redb").path
        var (profiles, renamed, firstGeneration) = try await prepareConnectionsConformance(path: path)
        let settings = profiles!.client()
        let ownerPath = root.appendingPathComponent("composed").path
        var malformedHost = connectionHost(); malformedHost.identity = .existing(secret: Data([1]))
        do {
            _ = try ServiceRuntime.createNewWithConnections(path: ownerPath, host: malformedHost,
                options: options(), profiles: profiles!)
            throw ConnectionConformanceFailure.failed("checked nested host input accepted")
        } catch BindingError.InvalidInput { }
        try connectionRequire(!files.fileExists(atPath: ownerPath), "composed malformed host caused effects")
        var owner: ServiceRuntime? = try ServiceRuntime.createNewWithConnections(path: ownerPath,
            host: connectionHost(), options: options(), profiles: profiles!)
        try await owner!.ready()
        try await owner!.networkReady()
        let composedBinding = try owner!.connectionBinding()
        let second = try await connectionApplied(composedBinding, renamed.guard.revision)
        try connectionRequire(second.generation != firstGeneration, "new host adopted an old binding generation")
        let shutdown = owner!.shutdown()
        try connectionRequire(composedBinding.status().phase != .active, "composed close did not fence profile reconciliation")
        owner = nil
        try await shutdown.wait()
        try connectionRequire(composedBinding.status().phase == .closed, "composed close preceded profile drain")
        let removed = try await settings.replace(expected: renamed.guard, profiles: [])
        owner = try ServiceRuntime.openExistingWithConnections(path: ownerPath,
            host: connectionHost(), options: options(), profiles: profiles!)
        try await owner!.ready()
        try await owner!.networkReady()
        let reopenedBinding = try owner!.connectionBinding()
        let restored = try await connectionApplied(reopenedBinding, removed.guard.revision)
        try connectionRequire(restored.attachments.isEmpty, "reopen reinserted an unwanted default")
        let finalClose = owner!.shutdown(); owner = nil
        try await finalClose.wait()

        try await finishConnectionsConformance(profiles: &profiles, path: path, expected: removed)
        print("PRNS_CONNECTIONS_SWIFT_OK: independent host journey plus composed generations, retained drain, no default reinsertion")
    }
}

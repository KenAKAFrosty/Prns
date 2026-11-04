private func transition(_ catalog: IdentityCatalogClient, _ identity: IdentityClient,
                        _ target: IdentityTransitionTarget) async throws -> IdentityOperationResult {
    var prepared: IdentityPreparedTransition? = nil
    for _ in 0..<500 {
        do { prepared = try await identity.prepare(target: target); break }
        catch { try await Task.sleep(nanoseconds: 10_000_000) }
    }
    guard let prepared else { throw Failure.failed("transition preparation did not settle") }
    let token = try await catalog.reserve(prepared: prepared, digest: Data(repeating: 9, count: 32))
    _ = try await identity.begin(token: token, prepared: prepared)
    for _ in 0..<500 {
        if case .completed(let result) = try await catalog.operation(token: token, command: .transition(prepared)) { return result }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    throw Failure.failed("transition did not settle")
}

@main
struct ComposedConformance {
    static func main() async throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-composed-swift-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        defer { try? files.removeItem(at: root) }
        let path = root.appendingPathComponent("owner").path
        var invalid = options()
        invalid.queueCapacity = 0
        do {
            _ = try ServiceRuntime.createNew(path: path, host: host(), options: invalid)
            throw Failure.failed("invalid budget accepted")
        } catch BindingError.InvalidInput(let field, _) {
            try require(field == "serviceRuntimeOptions", "native options lost typed error")
        }
        try require(!files.fileExists(atPath: path), "invalid options caused filesystem effects")
        var malformedHost = host()
        malformedHost.identity = .existing(secret: Data([1]))
        do {
            _ = try ServiceRuntime.createNew(path: path, host: malformedHost, options: options())
            throw Failure.failed("malformed host input accepted")
        } catch BindingError.InvalidInput { }
        try require(!files.fileExists(atPath: path), "host validation caused filesystem effects")
        var owner: ServiceRuntime? = try ServiceRuntime.createNew(path: path, host: host(), options: options())
        try await owner!.ready()
        try await owner!.networkReady()
        try require(owner!.status().phase == .ready, "owner readiness")
        let borrowedHost = try owner!.host()
        try require(try await borrowedHost.snapshot().interfaces.isEmpty, "unexpected interface policy")
        let catalog = try owner!.identities()
        let scope = try await catalog.creationScope()
        let request = IdentityCreateRequestId(scope: scope, nonce: Data(repeating: 1, count: 32))
        let intent = IdentityCreateRequest(source: .generate,
            metadata: IdentityMetadata(localLabel: "Roof sensor", publishedName: "Roof"), services: [])
        _ = try await catalog.createOrRecover(id: request, intent: intent, import: nil)
        let record = try await catalog.identities()[0]
        let reference = IdentityReference(id: record.id, publicHash: record.publicHash)
        let identity = try catalog.identity(reference: reference)
        do {
            _ = try owner!.groups(reference: IdentityReference(id: record.id, publicHash: Data([1])))
            throw Failure.failed("malformed identity reference accepted")
        } catch IdentityApiError.InvalidInput { }
        do {
            _ = try await identity.updateMetadata(expected: IdentityMetadataGuard(id: record.id,
                publicHash: record.publicHash, metadataRevision: record.metadataRevision + 1),
                metadata: record.metadata)
            throw Failure.failed("stale metadata guard accepted")
        } catch IdentityApiError.Authority(let reason) { try require(reason == .staleMetadata, "identity diagnostic reason") }
        let diagnostic = owner!.status().latestIdentityDiagnostic
        try require(diagnostic?.identity == reference && diagnostic?.error == .Authority(reason: .staleMetadata), "canonical retained diagnostic projection")
        try require(owner!.status().phase == .ready, "historical diagnostic changed owner phase")
        let card = try await identity.contactCard(serviceNameHash: serviceNameHash(dottedName: "prns.groups.enrollment"), displayName: "Roof")
        let claims = try verifyContactCard(wire: card)
        try require(claims.identityHash == record.publicHash, "canonical signed claim")
        try require(try await transition(catalog, identity, .addService(0x67727073)) == .serviceAdded(identity: record.id, service: 0x67727073), "explicit service setup")
        try require(try await transition(catalog, identity, .enable) == .enabled(identity: record.id), "explicit activation")
        let groups = try owner!.groups(reference: reference)
        let observation = try await groups.subscribe()
        let pageLimits = GroupPageLimits(maxRows: 8, maxBytes: 16 << 10)
        try require(try await groups.conversations(after: nil, limit: pageLimits).items.isEmpty, "initial group snapshot")
        let now = UInt64(Date().timeIntervalSince1970)
        let context = GroupExecutionContext(clock: .trusted(GroupClockBounds(lower: now, upper: now + 30)))
        let operation = try createRequestId()
        let accepted = try await groups.submit(id: operation,
            intent: .create(title: "Telemetry peers", profile: defaultGroupProfile()), context: context)
        try await observation.next()
        try require(try await groups.conversations(after: nil, limit: pageLimits).items.count == 1, "ordinary group mutation")
        try require(try await groups.operation(id: operation) == accepted, "retained group result")
        let group = try await groups.conversations(after: nil, limit: pageLimits).items[0].group
        let detail = try await groups.conversation(group: group, limit: pageLimits)
        let members = try await groups.members(group: group, after: nil, limit: pageLimits)
        try require(detail.memberCount == 1 && members.members.count == 1 &&
            members.control == detail.control && members.next == nil, "exact detail and paged members")
        let text = try textPublication(group: group, text: "Roof 🌡️")
        try require(text.group == group && text.namespace == 0x74657874 && text.schema == 1 &&
            text.targets == .otherMembers, "owning optional text profile")
        for invalidText in ["", String(repeating: "é", count: 1025)] {
            do { _ = try textPublication(group: group, text: invalidText); throw Failure.failed("invalid text accepted") }
            catch GroupApiError.InvalidInput { }
        }
        let genericPage = try await groups.records(group: group, before: nil, limit: 10, clock: context.clock)
        let textPage = try await groups.messages(group: group, before: nil, limit: 10, clock: context.clock)
        try require(textPage.messages.isEmpty && textPage.currentIncarnation == genericPage.currentIncarnation,
            "optional text view diverged from generic history")
        for invalidLimit: UInt32 in [0, 101, UInt32.max] {
            do { _ = try await groups.messages(group: group, before: nil, limit: invalidLimit, clock: context.clock)
                throw Failure.failed("unbounded text page accepted") }
            catch GroupApiError.InvalidInput { }
        }
        let close = owner!.shutdown()
        do {
            _ = try await borrowedHost.execute(command: .attachTcpServer(bind: "127.0.0.1:0", bitrate: .auto))
            throw Failure.failed("close did not fence borrowed host mutation")
        } catch BindingError.Stopped { }
        do {
            _ = try await borrowedHost.closeRemoteControlPairing()
            throw Failure.failed("close did not fence generated RemoteControl admission")
        } catch BindingError.Stopped { }
        owner = nil
        do { _ = try await catalog.identities(); throw Failure.failed("close did not fence catalog") }
        catch IdentityApiError.Submission(let reason) { try require(reason == .closed, "catalog close error") }
        try await close.wait()
        try await close.wait()
        try require(close.status().cleanupComplete, "close preceded resource release")
        try require(close.status().latestIdentityDiagnostic != nil, "drain erased historical diagnostics")
        do { _ = try await borrowedHost.snapshot(); throw Failure.failed("host outlived owner close") }
        catch BindingError.Stopped { }
        owner = try ServiceRuntime.openExisting(path: path, host: host(), options: options())
        try await owner!.ready()
        try await owner!.networkReady()
        let reopened = try owner!.identities()
        let snapshot = try await reopened.identity(reference: reference).snapshot()
        try require(snapshot.identity.desired == .enabled && snapshot.runtime == .unavailable, "reopen implicitly activated service")
        let history = try owner!.groups(reference: reference)
        try require(try await history.conversations(after: nil, limit: pageLimits).items.count == 1, "ordinary history lost on reopen")
        try require(try await history.operation(id: operation) == accepted, "retained result lost on reopen")
        let finalHost = try owner!.host()
        weak var released = owner
        owner = nil // foreign finalization also fences and retains native cleanup
        try require(released == nil, "foreign wrapper retained by borrowed clients")
        released = nil
        do { _ = try await reopened.identities(); throw Failure.failed("finalization did not fence catalog") }
        catch IdentityApiError.Submission(let reason) { try require(reason == .closed, "finalization error") }
        var hostStopped = false
        for _ in 0..<500 {
            do { _ = try await finalHost.snapshot() }
            catch BindingError.Stopped { hostStopped = true; break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        try require(hostStopped, "finalization failed to stop the borrowed host")
        owner = try ServiceRuntime.openExisting(path: path, host: host(), options: options())
        try await owner!.ready()
        try await owner!.networkReady()
        try require(try await owner!.groups(reference: reference).conversations(after: nil, limit: pageLimits).items.count == 1, "finalizer lost durable history")
        let finalClose = owner!.shutdown()
        owner = nil
        try await finalClose.wait()
        print("PRNS_NATIVE_COMPOSED_SWIFT_OK: paths/config, retained readiness, canonical clients, signed claim, explicit activation, group operation, restart, finalization, local-before-host close")
    }
}

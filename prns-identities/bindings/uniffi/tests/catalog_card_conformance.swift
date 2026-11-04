import PrnsIdentityCatalog

private func invalid(_ body: () async throws -> Void) async throws {
    do {
        try await body()
        throw CardFailure.failed("invalid card input accepted")
    } catch IdentityApiError.InvalidInput {}
}
private func options() -> IdentityOwnerOptions {
    IdentityOwnerOptions(queueCapacity: 8, budget: IdentityBudget(
        capacity: IdentityResources(heldKeys: 4, destinations: 4, requestHandlers: 4, links: 4, queueSlots: 16),
        infrastructureReserve: IdentityResources(heldKeys: 0, destinations: 0, requestHandlers: 0, links: 0, queueSlots: 0)))
}

@main
struct CatalogCardConformance {
    static func main() async throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-swift-catalog-card-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: root) }
        let path = root.appendingPathComponent("catalog").path
        var owner: IdentityCatalogOwner? = try IdentityCatalogOwner.createNew(path: path, options: options())
        let client = owner!.client()
        let scope = try await client.creationScope()
        let request = IdentityCreateRequestId(scope: scope, nonce: Data(repeating: 37, count: 32))
        let intent = IdentityCreateRequest(source: .generate,
            metadata: IdentityMetadata(localLabel: "Weather station", publishedName: ""), services: [])
        _ = try await client.createOrRecover(id: request, intent: intent, import: nil)
        let records = try await client.identities()
        try require(records.count == 1, "catalog creation")
        let record = records[0]
        let reference = IdentityReference(id: record.id, publicHash: record.publicHash)
        let identity = try client.identity(reference: reference)
        let before = try await identity.snapshot()
        try require(before.identity.desired == .paused && before.identity.services.isEmpty, "identity unexpectedly configured or enabled")
        let service = serviceHash("example.telemetry")
        try require(service == Data(SHA256.hash(data: Data("example.telemetry".utf8)).prefix(10)), "canonical service hash")
        let name = "Roof sensor 🌦"
        let card = try await identity.contactCard(serviceNameHash: service, displayName: name)
        try verifyCard(card, identity: record.publicHash, service: service, name: name)
        try require(try await identity.contactCard(serviceNameHash: service, displayName: name) == card, "same claim changed bytes")
        let other = serviceHash("example.inventory")
        try verifyCard(try await identity.contactCard(serviceNameHash: other, displayName: ""), identity: record.publicHash, service: other, name: "")
        for count in [0, 9, 11] {
            try await invalid { _ = try await identity.contactCard(serviceNameHash: Data(repeating: 1, count: count), displayName: name) }
        }
        for invalidName in ["bad\nname", "bad\u{202E}name", String(repeating: "x", count: 129)] {
            try await invalid { _ = try await identity.contactCard(serviceNameHash: service, displayName: invalidName) }
        }
        try require(try await identity.snapshot() == before, "offline signing changed lifecycle/catalog state")
        let close = owner!.shutdown()
        owner = nil
        try await close.wait()
        let reopened = try IdentityCatalogOwner.openExisting(path: path, options: options())
        let reopenedIdentity = try reopened.client().identity(reference: reference)
        try require(try await reopenedIdentity.contactCard(serviceNameHash: service, displayName: name) == card, "reopen changed exact public claim")
        let after = try await reopenedIdentity.snapshot()
        try require(after.identity.desired == .paused && after.identity.services.isEmpty, "reopen/signing activated a service")
        try await reopened.shutdown().wait()
        print("PRNS_IDENTITY_CATALOG_CARD_SWIFT_OK: canonical generated method, checked claims, paused/no-services issuance, independent signature verification, restart")
    }
}

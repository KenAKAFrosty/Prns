import Foundation
import PrnsIdentityCatalog

private enum ConformanceFailure: Error { case failed(String) }
private func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ConformanceFailure.failed(message) }
}
private func expect(_ expected: IdentityApiError, _ body: () async throws -> Void) async throws {
    do {
        try await body()
        throw ConformanceFailure.failed("operation unexpectedly succeeded")
    } catch let error as IdentityApiError {
        try require(error == expected, "unexpected typed error: \(error)")
    }
}
private func completed(_ client: IdentityCatalogClient, _ token: IdentityOperationToken,
                       _ prepared: IdentityPreparedTransition) async throws -> IdentityOperationResult {
    for _ in 0..<200 {
        if case .completed(let result) = try await client.operation(token: token, command: .transition(prepared)) {
            return result
        }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    throw ConformanceFailure.failed("lifecycle result did not settle")
}
private func options(_ queue: UInt64 = 8) -> IdentityOwnerOptions {
    IdentityOwnerOptions(queueCapacity: queue, budget: IdentityBudget(
        capacity: IdentityResources(heldKeys: 4, destinations: 4, requestHandlers: 4, links: 4, queueSlots: 16),
        infrastructureReserve: IdentityResources(heldKeys: 0, destinations: 0, requestHandlers: 0, links: 0, queueSlots: 0)))
}

@main
struct CatalogConformance {
    static func main() async throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-swift-catalog-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: root) }
        let invalid = root.appendingPathComponent("invalid").path
        try await expect(.InvalidOptions) {
            _ = try IdentityCatalogOwner.createNew(path: invalid, options: options(0))
        }
        try require(!files.fileExists(atPath: invalid), "invalid options touched filesystem")
        let path = root.appendingPathComponent("catalog").path
        var owner: IdentityCatalogOwner? = try IdentityCatalogOwner.createNew(path: path, options: options())
        let client = owner!.client()
        let scope = try await client.creationScope()
        let request = IdentityCreateRequestId(scope: scope, nonce: Data(repeating: 7, count: 32))
        let intent = IdentityCreateRequest(source: .generate,
            metadata: IdentityMetadata(localLabel: "Weather station", publishedName: ""), services: [])
        let created = try await client.createOrRecover(id: request, intent: intent, import: nil)
        guard case .created(let localId) = created.result else { throw ConformanceFailure.failed("creation result") }
        try require(try await client.createOrRecover(id: request, intent: intent, import: nil) == created, "exact retry changed creation")
        var changed = intent
        changed.metadata.localLabel = "Different intent"
        try await expect(.Creation(reason: .conflict)) {
            _ = try await client.createOrRecover(id: request, intent: changed, import: nil)
        }
        try await expect(.InvalidInput) {
            _ = try await client.createOrRecover(id: IdentityCreateRequestId(scope: scope, nonce: Data(repeating: 1, count: 31)), intent: intent, import: nil)
        }
        var tooMany = intent
        tooMany.services = [1, 2, 3, 4, 5]
        try await expect(.Submission(reason: .bounds)) {
            _ = try await client.createOrRecover(id: IdentityCreateRequestId(scope: scope, nonce: Data(repeating: 2, count: 32)), intent: tooMany, import: nil)
        }
        let records = try await client.identities()
        try require(records.count == 1 && records[0].id == localId, "canonical collection projection")
        let record = records[0]
        let reference = IdentityReference(id: record.id, publicHash: record.publicHash)
        let identity = try client.identity(reference: reference)
        let guardValue = IdentityMetadataGuard(id: record.id, publicHash: record.publicHash, metadataRevision: record.metadataRevision)
        let renamed = IdentityMetadata(localLabel: "Roof telemetry", publishedName: "Node 7")
        _ = try await identity.updateMetadata(expected: guardValue, metadata: renamed)
        try require(try await identity.snapshot().identity.metadata == renamed, "canonical metadata mutation")
        try await expect(.Authority(reason: .staleMetadata)) {
            _ = try await identity.updateMetadata(expected: guardValue, metadata: renamed)
        }
        let malformed = IdentityReference(id: record.id, publicHash: Data(repeating: 0, count: 15))
        try await expect(.InvalidInput) { _ = try client.identity(reference: malformed) }
        let prepared = try await identity.prepare(target: .enable)
        let digest = Data(repeating: 19, count: 32)
        let token = try await client.reserve(prepared: prepared, digest: digest)
        try require(try await client.findReserved(command: .transition(prepared), digest: digest) == token, "reservation exact identity")
        _ = try await identity.begin(token: token, prepared: prepared)
        try require(try await completed(client, token, prepared) == .enabled(identity: record.id), "standalone enable")
        try require(try await identity.snapshot().runtime == .running, "explicit activation")
        let close = owner!.shutdown()
        owner = nil
        try await close.wait()
        try await expect(.Submission(reason: .closed)) { _ = try await client.identities() }
        owner = try IdentityCatalogOwner.openExisting(path: path, options: options())
        let reopened = owner!.client()
        let reopenedIdentity = try reopened.identity(reference: reference)
        let snapshot = try await reopenedIdentity.snapshot()
        try require(snapshot.identity.desired == .enabled && snapshot.runtime == .unavailable, "reopen activated saved state")
        try require(snapshot.identity.metadata == renamed, "reopen changed metadata")
        try require(try await reopened.createOrRecover(id: request, intent: intent, import: nil) == created, "restart changed retained creation evidence")
        try await reopenedIdentity.recover()
        let pause = try await reopenedIdentity.prepare(target: .pause)
        let stopped = try await reopenedIdentity.applyStop(prepared: pause, digest: Data(repeating: 20, count: 32))
        try require(try await completed(reopened, stopped, pause) == .paused(identity: record.id), "pause did not drain")

        // Import uses an opaque retained capability from the already-qualified
        // key owner. No secret representation crosses this consumer boundary.
        let keyPath = root.appendingPathComponent("key").path
        var keyOwner: KeyOwner? = try KeyOwner.createNew(path: keyPath)
        let expected = keyOwner!.identityHash()
        var key: IdentityImportKey? = keyOwner!.lease().importKey()
        keyOwner = nil
        do {
            _ = try KeyOwner.openExisting(path: keyPath, expectedIdentity: expected)
            throw ConformanceFailure.failed("import capability lost source lease")
        } catch KeyError.Busy {}
        let importedIntent = IdentityCreateRequest(source: .import(expectedHash: expected),
            metadata: IdentityMetadata(localLabel: "Imported sensor", publishedName: ""), services: [])
        _ = try await reopened.createOrRecover(id: IdentityCreateRequestId(scope: scope, nonce: Data(repeating: 8, count: 32)), intent: importedIntent, import: key)
        // The following lane read also observes completion of the import job's
        // stack cleanup before testing last-capability lock release.
        let importedRecords = try await reopened.identities()
        key = nil
        try require(try KeyOwner.openExisting(path: keyPath, expectedIdentity: expected).identityHash() == expected, "source lease did not release after import")
        try require(importedRecords.contains { $0.publicHash == expected }, "import changed public identity")
        let finalClose = owner!.shutdown()
        owner = nil
        try await finalClose.wait()
        print("PRNS_IDENTITY_CATALOG_SWIFT_OK: canonical methods, checked models, exact creation replay, scoped lifecycle, restart, opaque import, retained close")
    }
}

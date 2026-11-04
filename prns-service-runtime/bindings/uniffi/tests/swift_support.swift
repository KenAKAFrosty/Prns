import Foundation
import PrnsServiceRuntimeImage

private enum Failure: Error { case failed(String) }
private func require(_ value: Bool, _ message: String) throws {
    if !value { throw Failure.failed(message) }
}
private func host() -> HostConfig {
    HostConfig(identity: .generateEphemeral, persistence: .ephemeral, role: .endpoint,
        destinations: [], requiredCapabilities: [], limits: PrnsLimits(
            pendingCommands: 64, applicationEvents: 64, retainedEventBytes: 1_048_576, diagnostics: 32))
}
private func options() -> ServiceRuntimeOptions {
    ServiceRuntimeOptions(startupRecovery: .dormant, queueCapacity: 32,
        identityBudget: IdentityBudget(capacity: IdentityResources(heldKeys: 4, destinations: 8,
            requestHandlers: 8, links: 32, queueSlots: 256), infrastructureReserve: IdentityResources(
                heldKeys: 0, destinations: 0, requestHandlers: 0, links: 0, queueSlots: 0)),
        cacheBytesPerIdentity: 1 << 20, totalCacheBytes: 4 << 20,
        groupClient: GroupClientLimits(maxJobBytes: 1 << 20, maxPendingJobBytes: 8 << 20),
        validationCacheEntries: 4,
        retention: GroupRetentionLimits(maxOperations: 32, maxBytes: 2 << 20, maxRowBytes: 128 << 10, maxOperationBytes: 128 << 10, maxPruneRows: 32),
        catalog: GroupCatalogLimits(maxGroups: 4, maxCatalogBytes: 2 << 20, maxEnrollmentFrameBytes: 131072,
            maxRowBytes: 16 << 10, maxTransactionRows: 128, maxTransactionBytes: 4 << 20,
            maxMaintenanceRows: 32, maxIncomingInvitations: 4, maxIncomingPerPeer: 2,
            namespace: GroupStoreLimits(maxChanges: 128, maxKeyBytes: 256, maxValueBytes: 2 << 20,
                maxStagedBytes: 4 << 20, maxLiveObjects: 8192, maxLiveBytes: 16 << 20, maxReceipts: 4096)),
        archive: GroupArchiveLimits(maxRecords: 256, maxBytes: 8 << 20),
        carrier: ServiceCarrierOptions(operations: 16, outgoingLinks: 8, syncFrameBytes: 131072,
            syncPageItems: 8, syncPeers: 4, syncInboundJobs: 16, syncTimeoutMillis: 5000,
            enrollmentFrameBytes: 131072, enrollmentInboundJobs: 4, enrollmentTimeoutMillis: 5000),
        syncPolicy: GroupSyncPolicy(minExchangeMillis: 10000, maxExchangeMillis: 600000,
            unknownBitrateBps: 1200, unknownRttMillis: 2000, fastBitrateBps: 64000,
            fastRttMillis: 500, fastPollMillis: 2000, constrainedPollMillis: 15000,
            maxFastIdleMillis: 10000, maxIdleMillis: 300000, announceIntervalMillis: 21600000),
        announceIntervalMillis: nil)
}

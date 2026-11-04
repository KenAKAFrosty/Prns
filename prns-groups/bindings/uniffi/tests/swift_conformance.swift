import Foundation
import PrnsGroupServices

private enum Failure: Error { case failed(String) }
private func require(_ value: Bool, _ message: String) throws {
    if !value { throw Failure.failed(message) }
}
private func expect(_ expected: GroupApiError, _ body: () async throws -> Void) async throws {
    do { try await body(); throw Failure.failed("operation unexpectedly succeeded") }
    catch let error as GroupApiError { try require(error == expected, "expected \(expected), got \(error)") }
}

@main
struct GroupConformance {
    static func main() async throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-swift-groups-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        defer { try? files.removeItem(at: root) }
        let keyPath = root.appendingPathComponent("identity").path
        let groupPath = root.appendingPathComponent("groups.redb").path
        var keyOwner: KeyOwner? = try KeyOwner.createNew(path: keyPath)
        let identity = keyOwner!.identityHash()
        var lease: KeyLease? = keyOwner!.lease()
        do {
            _ = try GroupLocalOwner.createNew(path: "invalid\0path", key: lease!, queueCapacity: 1025, cacheBytes: 1 << 20)
            throw Failure.failed("native resource budget was not checked")
        } catch GroupOpenError.InvalidOptions { }
        var owner: GroupLocalOwner? = try GroupLocalOwner.createNew(path: groupPath, key: lease!, queueCapacity: 8, cacheBytes: 1 << 20)
        let client = owner!.client()
        weak var releasedKey = keyOwner
        weak var releasedLease = lease
        keyOwner = nil
        lease = nil
        try require(releasedKey == nil && releasedLease == nil, "foreign key wrappers were retained")
        releasedKey = nil
        releasedLease = nil
        try require(try await client.identity() == identity, "group identity binding")
        do {
            _ = try KeyOwner.openExisting(path: keyPath, expectedIdentity: identity)
            throw Failure.failed("worker did not retain key lock")
        } catch KeyError.Busy { }

        let observation = try await client.subscribe() // before the initial snapshot
        let query = GroupPageLimits(maxRows: 8, maxBytes: 1 << 20)
        let initial = try await client.conversations(after: nil, limit: query)
        try require(initial.items.isEmpty && initial.next == nil, "initial snapshot")
        let now = UInt64(Date().timeIntervalSince1970)
        let context = GroupExecutionContext(clock: .trusted(GroupClockBounds(lower: now, upper: now + 30)))
        let unknown = GroupExecutionContext(clock: .unknown)
        let request = GroupRequest(id: try createRequestId(), intent: .create(title: "Sensor team", profile: defaultGroupProfile()), originalScope: nil)
        let result = try await client.execute(request: request, context: context)
        try await observation.next()
        guard case let .created(group) = result else { throw Failure.failed("create outcome") }
        try require(try await client.execute(request: request, context: unknown) == result, "exact replay with unknown time")
        var conflicting = request
        conflicting.intent = .create(title: "Different intent", profile: defaultGroupProfile())
        try await expect(.Rejected(reason: .requestConflict)) {
            _ = try await client.execute(request: conflicting, context: unknown)
        }
        let conversations = try await client.conversations(after: nil, limit: query)
        try require(conversations.items.count == 1 && conversations.items[0].memberCount == 1 && conversations.next == nil, "canonical conversation page")
        let detail = try await client.conversation(group: group, limit: query)
        let members = try await client.members(group: group, after: nil, limit: query)
        try require(detail.profile == defaultGroupProfile() && detail.memberCount == 1 &&
            members.members.count == 1 && members.next == nil && members.control == detail.control, "exact detail and member page")
        let scope = try await client.groupScope(group: group)
        guard case let .control(control, incarnation) = scope else { throw Failure.failed("missing signed scope") }
        try require(control == detail.control && incarnation.count == 24, "canonical control and exact store incarnation")
        try require(try await client.invitations(now: now, after: nil, limit: query).items.isEmpty, "initial invitations")
        let recipient = Data(repeating: 6, count: 16)
        let invite = GroupRequest(id: try createRequestId(), intent: .invite(group: group, recipient: recipient), originalScope: scope)
        let invited = try await client.execute(request: invite, context: context)
        guard case let .invited(invitation) = invited else { throw Failure.failed("invite outcome") }
        let invitationPage = try await client.invitations(now: now, after: nil, limit: query)
        try require(invitationPage.next == nil, "complete tiny invitation page")
        let invitations = invitationPage.items
        try require(invitations.count == 1 && invitations[0].id == invitation &&
            invitations[0].group == group && invitations[0].recipient == recipient &&
            invitations[0].state == .pending && invitations[0].outgoing && invitations[0].canCancel,
            "canonical invitation snapshot")
        _ = try await client.execute(request: GroupRequest(id: try createRequestId(),
            intent: .cancelInvitation(invitation: invitation), originalScope: nil), context: context)
        let cancelled = try await client.invitations(now: now, after: nil, limit: query).items
        try require(cancelled.count == 1 && cancelled[0].state == .cancelled && cancelled[0].canDismiss,
            "cancelled invitation snapshot")
        _ = try await client.execute(request: GroupRequest(id: try createRequestId(),
            intent: .dismissInvitation(invitation: invitation), originalScope: nil), context: context)
        try require(try await client.invitations(now: now, after: nil, limit: query).items.isEmpty, "dismissed invitation snapshot")
        let page = try await client.records(group: group, before: nil, limit: 10, clock: context.clock)
        try require(page.records.isEmpty && page.currentIncarnation.count == 24, "bounded record page")
        let changes = try await client.changes(group: group, after: nil, limit: 10)
        try require(changes.head.group == group && changes.head.incarnation == page.currentIncarnation, "canonical feed scope")
        let absent = Data(repeating: 0xa5, count: 32)
        try require(try await client.record(group: group, incarnation: page.currentIncarnation, record: absent, clock: context.clock) == nil, "absent generic record")
        try require(try await client.deliveryProgress(group: group, incarnation: page.currentIncarnation, record: absent, clock: context.clock) == nil, "absent delivery evidence")

        // These malformed values are representable in generated Swift. They
        // must fail while lifting the actual Rust types as GroupApiError.
        try await expect(.InvalidInput) { _ = try await client.groupScope(group: Data([1])) }
        try await expect(.InvalidInput) { _ = try await client.conversations(after: nil, limit: GroupPageLimits(maxRows: 0, maxBytes: 1 << 20)) }
        var malformed = request
        malformed.id = Data([1])
        try await expect(.InvalidInput) { _ = try await client.execute(request: malformed, context: context) }
        try await expect(.InvalidInput) {
            _ = try await client.execute(request: request, context: GroupExecutionContext(clock: .trusted(GroupClockBounds(lower: now + 1, upper: now))))
        }
        try await expect(.InvalidInput) {
            _ = try await client.record(group: group, incarnation: Data(repeating: 0, count: 24), record: absent, clock: context.clock)
        }
        let malformedMember = GroupEndpoint(id: absent, incarnation: 0)
        let removal = GroupRequest(id: try createRequestId(), intent: .removeMember(group: group, member: malformedMember), originalScope: scope)
        try await expect(.InvalidInput) { _ = try await client.execute(request: removal, context: context) }
        let invalidIdentity = GroupRequest(id: try createRequestId(), intent: .invite(group: group, recipient: Data([1])), originalScope: scope)
        try await expect(.InvalidInput) { _ = try await client.execute(request: invalidIdentity, context: context) }
        for limit: UInt32 in [0, 101, UInt32.max] {
            try await expect(.InvalidInput) { _ = try await client.records(group: group, before: nil, limit: limit, clock: context.clock) }
        }
        let invalidToken = GroupRequest(id: Data(repeating: 0xa5, count: 32), intent: .create(title: "Invalid token", profile: defaultGroupProfile()), originalScope: nil)
        try await expect(.Retry(reason: .invalidToken)) { _ = try await client.execute(request: invalidToken, context: context) }
        let noRecipients = GroupRequest(id: try createRequestId(), intent: .publish(GroupPublicationIntent(group: group, namespace: 0x74656c65, schema: 1, targets: .otherMembers, body: Data([0, 255, 1]))), originalScope: scope)
        try await expect(.Rejected(reason: .noRecipients)) { _ = try await client.execute(request: noRecipients, context: context) }

        let closed = owner!.shutdown() // admission is fenced before wait starts
        owner = nil
        try await expect(.Unavailable) { _ = try await client.identity() }
        try await expect(.Unavailable) { try await observation.next() }
        try await closed.wait()
        try await closed.wait() // retained result, not a one-shot channel
        keyOwner = try KeyOwner.openExisting(path: keyPath, expectedIdentity: identity)
        owner = try GroupLocalOwner.openExisting(path: groupPath, key: keyOwner!.lease(), queueCapacity: 8, cacheBytes: 1 << 20)
        let reopened = owner!.client()
        try require(try await reopened.execute(request: request, context: unknown) == result, "durable exact replay after reopen")
        weak var releasedOwner = owner
        owner = nil // finalization also fences borrowed clients without waiting
        try require(releasedOwner == nil, "foreign group owner was not released")
        releasedOwner = nil
        keyOwner = nil
        try await expect(.Unavailable) { _ = try await reopened.identity() }
        // The worker releases its key only after database cleanup. No runtime
        // inside the native image drives this wait; Swift merely observes it.
        for _ in 0..<1000 {
            do { keyOwner = try KeyOwner.openExisting(path: keyPath, expectedIdentity: identity); break }
            catch KeyError.Busy { try await Task.sleep(nanoseconds: 10_000_000) }
        }
        try require(keyOwner != nil, "finalized group retained its key after cleanup")
        owner = try GroupLocalOwner.openExisting(path: groupPath, key: keyOwner!.lease(), queueCapacity: 8, cacheBytes: 1 << 20)
        let cleanup = owner!.shutdown()
        owner = nil
        try await cleanup.wait()
        keyOwner = nil
        print("PRNS_GROUP_SERVICES_SWIFT_OK: canonical requests, checked lifting, key retention, ordinary client, close and recovery")
    }
}

import Foundation
import PrnsChatAppImage

enum ImageFailure: Error { case versionMismatch, unexpectedIdentity, controllerChanged, retainedResultMissing, invalidGroupPolicy }

@main struct ImageConformance {
    private static func startAndRelease(_ path: URL) async throws -> Data {
        let owner = try appStartNew(storageRoot: path.path)
        try await owner.ready()
        guard try await owner.identities().identities().isEmpty else {
            throw ImageFailure.unexpectedIdentity
        }
        try await owner.networkReady()
        return try owner.host().identityHash()
    }

    static func main() async throws {
        guard CommandLine.arguments.count == 2,
              let expected = UInt32(CommandLine.arguments[1]),
              appApiVersion() == expected else {
            throw ImageFailure.versionMismatch
        }
        let policy = applicationOptions()
        let profile = try applicationGroupProfile(maxMembers: 256)
        guard policy.catalog.maxGroups == nil,
              profile.maxMembers == 256,
              profile.maxEnrollmentFrameBytes <= policy.carrier.enrollmentFrameBytes,
              policy.catalog.namespace.maxStagedBytes >= policy.catalog.namespace.maxValueBytes else {
            throw ImageFailure.invalidGroupPolicy
        }
        do {
            _ = try applicationGroupProfile(maxMembers: 0)
            throw ImageFailure.invalidGroupPolicy
        } catch GroupApiError.InvalidInput { }
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("prns-app-image-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: path, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: path) }
        let originalHash = try await startAndRelease(path)
        // The only construction wrapper left its scope. The process still owns
        // the exact native owner, just as it does across a JS context reload.
        let borrowed = try appOwner()
        guard try borrowed.host().identityHash() == originalHash else {
            throw ImageFailure.controllerChanged
        }
        let operation: UInt64
        do {
            let operations = try appRemoteControlOperations()
            operation = try operations.submitLink(client: borrowed.host(),
                linkId: Data(repeating: 0xAA, count: 16), request: .describe)
            guard let result = await operations.wait(operationId: operation),
                  case .notSubmitted = result.state else {
                throw ImageFailure.retainedResultMissing
            }
        }
        // Releasing every operation wrapper and stopping the host leaves the
        // same process-owned result available through a fresh ordinary accessor.
        try await appStop().wait()
        guard borrowed.networkStatus().phase == .offline,
              try await borrowed.identities().identities().isEmpty else {
            throw ImageFailure.unexpectedIdentity
        }
        guard let result = try appRemoteControlOperations().operation(operationId: operation),
              case .notSubmitted = result.state else {
            throw ImageFailure.retainedResultMissing
        }
        let reopened = try appOpenExisting(storageRoot: path.path)
        try await reopened.ready()
        try await reopened.networkReady()
        guard try reopened.host().identityHash() == originalHash else {
            throw ImageFailure.controllerChanged
        }
        guard try await reopened.identities().identities().isEmpty else {
            throw ImageFailure.unexpectedIdentity
        }
        try await appStop().wait()
        guard try appRemoteControlOperations().retire(operationId: operation) == .retired else {
            throw ImageFailure.retainedResultMissing
        }
        let review = try appPrepareReset()
        try await appReset(review: review)
        print("App image: API version, canonical group/store policies, retained bootstrap, offline local reads, controller identity, reopen and reviewed reset passed.")
    }
}

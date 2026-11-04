import Foundation
import PrnsIdentityKeys

private enum ConformanceFailure: Error { case failed(String) }
private func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ConformanceFailure.failed(message) }
}
private func expect(_ check: (KeyError) -> Bool, _ body: () throws -> Void) throws {
    do {
        try body()
        throw ConformanceFailure.failed("operation unexpectedly succeeded")
    } catch let error as KeyError {
        try require(check(error), "unexpected typed error: \(error)")
    }
}

@main
struct KeyConformance {
    static func main() throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-swift-keys-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: root) }
        let path = root.appendingPathComponent("identity").path
        var owner: KeyOwner? = try KeyOwner.createNew(path: path)
        weak var weakOwner = owner
        let expected = owner!.identityHash()
        try require(expected.count == 16, "public identity width")
        var lease: KeyLease? = owner!.lease()
        try require(lease!.identityHash() == expected, "retained identity")
        owner = nil
        try require(weakOwner == nil, "foreign owner was not released")
        weakOwner = nil
        try expect({ if case .Busy = $0 { return true }; return false }) {
            _ = try KeyOwner.openExisting(path: path, expectedIdentity: expected)
        }
        try require(lease!.identityHash() == expected, "lease must retain the native key")
        lease = nil
        var reopened: KeyOwner? = try KeyOwner.openExisting(path: path, expectedIdentity: expected)
        try require(reopened!.identityHash() == expected, "exact identity after last lease drop")
        reopened = nil
        try require(Set(try files.contentsOfDirectory(atPath: path)) == ["identity.key", "owner.lock"], "unexpected catalog or service state")

        try expect({ if case .InvalidIdentityHash = $0 { return true }; return false }) {
            _ = try KeyOwner.openExisting(path: "invalid\0path", expectedIdentity: Data(repeating: 0, count: 15))
        }
        var wrong = expected
        wrong[0] ^= 1
        try expect({ if case .IdentityMismatch = $0 { return true }; return false }) {
            _ = try KeyOwner.openExisting(path: path, expectedIdentity: wrong)
        }
        try require(try KeyOwner.openExisting(path: path, expectedIdentity: expected).identityHash() == expected, "mismatch changed identity")
        try expect({ if case .Io = $0 { return true }; return false }) {
            _ = try KeyOwner.createNew(path: path)
        }

        let alias = root.appendingPathComponent("alias").path
        try files.createSymbolicLink(atPath: alias, withDestinationPath: path)
        try expect({ if case .UnsafePath = $0 { return true }; return false }) {
            _ = try KeyOwner.openExisting(path: alias, expectedIdentity: expected)
        }
        try files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
        try expect({ if case .UnsafePermissions = $0 { return true }; return false }) {
            _ = try KeyOwner.openExisting(path: path, expectedIdentity: expected)
        }
        try files.setAttributes([.posixPermissions: 0o700], ofItemAtPath: path)

        // The observer records a test oracle, while the simulated caller loses
        // the creation handle before durably recording its public binding.
        let lostPath = root.appendingPathComponent("lost-creation").path
        var lost: KeyOwner? = try KeyOwner.createNew(path: lostPath)
        let observed = lost!.identityHash()
        lost = nil
        var recovered: KeyOwner? = try KeyOwner.recoverCreation(path: lostPath)
        let recoveredHash = recovered!.identityHash()
        try require(recoveredHash == observed, "recovery replaced initial identity")
        recovered = nil
        try require(try KeyOwner.openExisting(path: lostPath, expectedIdentity: recoveredHash).identityHash() == observed, "recovered identity did not reopen")
        let missing = root.appendingPathComponent("missing").path
        // Preserve the native I/O category: an OS error kind does not establish
        // whether an earlier publication step wrote state.
        try expect({ if case .Io = $0 { return true }; return false }) {
            _ = try KeyOwner.recoverCreation(path: missing)
        }
        try require(!files.fileExists(atPath: missing), "recovery generated missing identity")
        let missingKey = root.appendingPathComponent("missing-key").path
        var missingOwner: KeyOwner? = try KeyOwner.createNew(path: missingKey)
        try require(missingOwner!.identityHash().count == 16, "missing-key fixture creation")
        missingOwner = nil
        let missingFile = URL(fileURLWithPath: missingKey).appendingPathComponent("identity.key")
        try files.removeItem(at: missingFile)
        try expect({ if case .Missing = $0 { return true }; return false }) {
            _ = try KeyOwner.recoverCreation(path: missingKey)
        }
        try require(!files.fileExists(atPath: missingFile.path), "recovery generated missing key")

        let corrupt = root.appendingPathComponent("corrupt").path
        var corruptOwner: KeyOwner? = try KeyOwner.createNew(path: corrupt)
        let corruptHash = corruptOwner!.identityHash()
        corruptOwner = nil
        let keyFile = URL(fileURLWithPath: corrupt).appendingPathComponent("identity.key")
        let damaged = Data("incomplete-record".utf8)
        try damaged.write(to: keyFile)
        try expect({ if case .Corrupt = $0 { return true }; return false }) {
            _ = try KeyOwner.openExisting(path: corrupt, expectedIdentity: corruptHash)
        }
        try expect({ if case .Corrupt = $0 { return true }; return false }) {
            _ = try KeyOwner.recoverCreation(path: corrupt)
        }
        try require(try Data(contentsOf: keyFile) == damaged, "corrupt state was replaced")
        print("PRNS_IDENTITY_KEYS_SWIFT_OK: ownable paths, opaque lease retention, exact reopen, recovery, typed failures")
    }
}

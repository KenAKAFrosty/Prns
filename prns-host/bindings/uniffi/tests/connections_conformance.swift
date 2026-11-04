import Foundation
import PrnsHostBindings

@main
struct ConnectionsConformance {
    static func main() async throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-host-connections-swift-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        defer { try? files.removeItem(at: root) }
        let path = root.appendingPathComponent("profiles.redb").path
        var (profiles, saved, _) = try await prepareConnectionsConformance(path: path)
        let removed = try await profiles!.client().replace(expected: saved.guard, profiles: [])
        try await finishConnectionsConformance(profiles: &profiles, path: path, expected: removed)
        print("PRNS_HOST_CONNECTIONS_SWIFT_OK: checked inputs, strict paths, stopped CAS, independent host ownership, retained drain, strict reopen without default reinsertion")
    }
}

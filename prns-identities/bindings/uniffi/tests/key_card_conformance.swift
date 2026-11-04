import PrnsIdentityKeys

private func invalid(_ body: () throws -> Void) throws {
    do {
        try body()
        throw CardFailure.failed("invalid card input accepted")
    } catch KeyError.InvalidContactCard {}
}

@main
struct KeyCardConformance {
    static func main() throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("prns-swift-key-card-" + UUID().uuidString)
        try files.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? files.removeItem(at: root) }
        let path = root.appendingPathComponent("key").path
        var owner: KeyOwner? = try KeyOwner.createNew(path: path)
        let identity = owner!.identityHash()
        var lease: KeyLease? = owner!.lease()
        owner = nil
        let service = serviceHash("example.telemetry")
        try require(service == Data(SHA256.hash(data: Data("example.telemetry".utf8)).prefix(10)), "canonical service hash")
        let name = "Roof sensor 🌦"
        let card = try lease!.contactCard(serviceNameHash: service, displayName: name)
        try verifyCard(card, identity: identity, service: service, name: name)
        try require(try lease!.contactCard(serviceNameHash: service, displayName: name) == card, "same claim changed bytes")
        let other = serviceHash("example.inventory")
        try verifyCard(try lease!.contactCard(serviceNameHash: other, displayName: ""), identity: identity, service: other, name: "")
        for count in [0, 9, 11] {
            try invalid { _ = try lease!.contactCard(serviceNameHash: Data(repeating: 1, count: count), displayName: name) }
        }
        for invalidName in ["bad\nname", "bad\u{202E}name", String(repeating: "x", count: 129)] {
            try invalid { _ = try lease!.contactCard(serviceNameHash: service, displayName: invalidName) }
        }
        do {
            _ = try KeyOwner.openExisting(path: path, expectedIdentity: identity)
            throw CardFailure.failed("card lease released native ownership")
        } catch KeyError.Busy {}
        try require(Set(try files.contentsOfDirectory(atPath: path)) == Set(["identity.key", "owner.lock"]), "card issuance created catalog/service files")
        lease = nil
        let reopened = try KeyOwner.openExisting(path: path, expectedIdentity: identity)
        try verifyCard(try reopened.lease().contactCard(serviceNameHash: service, displayName: name), identity: identity, service: service, name: name)
        print("PRNS_IDENTITY_KEY_CARD_SWIFT_OK: opaque retained key, checked claims, canonical public wire, independent signature verification, no catalog")
    }
}

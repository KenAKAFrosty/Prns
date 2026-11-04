// Test-only bounded parsing of the canonical version-one public card wire.
// CryptoKit independently checks its identity hash and Ed25519 signature.
import Foundation
import CryptoKit

private enum CardFailure: Error { case failed(String) }
private func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw CardFailure.failed(message) }
}
private func serviceHash(_ dottedName: String) -> Data {
    serviceNameHash(dottedName: dottedName)
}
private func verifyCard(_ wire: Data, identity: Data, service: Data, name: String) throws {
    let uri = try encodeContactCardUri(wire: wire)
    try require(uri.hasPrefix("rns-contact://v1/"), "canonical URI prefix")
    let decoded = try decodeContactCardUri(uri: uri)
    try require(decoded == wire, "URI carrier changed signed bytes")
    do {
        _ = try decodeContactCardUri(uri: uri + "#extra")
        throw CardFailure.failed("URI decoder accepted normalized suffix")
    } catch is ContactCardTokenError {}
    let claims = try verifyContactCard(wire: wire)
    try require(claims.identityHash == identity && claims.serviceNameHash == service && claims.displayName == name, "canonical verified claims differ")
    var destinationInput = service
    destinationInput.append(identity)
    try require(claims.destinationHash == Data(SHA256.hash(data: destinationInput).prefix(16)), "canonical destination derivation")
    try require(claims.publicIdentity == Data(wire[10..<74]), "canonical public identity projection")
    var corrupted = wire
    corrupted[corrupted.count - 1] ^= 1
    do {
        _ = try verifyContactCard(wire: corrupted)
        throw CardFailure.failed("canonical verifier accepted tampered signature")
    } catch ContactCardError.InvalidSignature {}
    do {
        _ = try verifyContactCard(wire: Data())
        throw CardFailure.failed("canonical verifier accepted empty card")
    } catch ContactCardError.Truncated {}
    try require(wire.count >= 148 && wire.count <= 276, "card wire bounds")
    try require(Data(wire.prefix(8)) == Data("PRNSCARD".utf8) && wire[8] == 1, "card magic/version")
    let nameLength = Int(wire[9])
    let unsignedLength = 84 + nameLength
    try require(nameLength <= 128 && wire.count == unsignedLength + 64, "card exact length")
    let publicIdentity = Data(wire[10..<74])
    try require(Data(SHA256.hash(data: publicIdentity).prefix(16)) == identity, "card changed public identity")
    try require(Data(wire[74..<84]) == service, "card changed service claim")
    try require(Data(wire[84..<unsignedLength]) == Data(name.utf8), "card changed display name")
    let verifier = try Curve25519.Signing.PublicKey(rawRepresentation: Data(publicIdentity.suffix(32)))
    var message = Data("prns.contact-card.v1\0".utf8)
    message.append(wire.prefix(unsignedLength))
    let signature = Data(wire.suffix(64))
    try require(verifier.isValidSignature(signature, for: message), "card signature invalid")
    message[message.count - 1] ^= 1
    try require(!verifier.isValidSignature(signature, for: message), "signature accepted altered claim")
}

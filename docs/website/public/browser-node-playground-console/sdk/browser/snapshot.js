import { destinationHash, identityHash, interfaceId, } from "../contract.js";
import { bytesField, field, literalField, nonNegativeBigIntField, numberField, optionalArrayField, optionalBytesField, optionalNumber, record, stringField, } from "./decoding.js";
import { PrnsValidationError, bitrateBps, hardwareMtu, nonNegativeInteger, } from "./values.js";
export function parseSnapshot(raw) {
    const object = record(raw, "PrnsSnapshot");
    const interfacesRaw = field(object, "interfaces");
    if (!Array.isArray(interfacesRaw)) {
        throw new PrnsValidationError("invalid-component", "snapshot interfaces must be an array");
    }
    const routeSnapshotsRaw = optionalArrayField(object, "routeSnapshots");
    const destinationIdentitiesRaw = optionalArrayField(object, "destinationIdentities");
    return {
        type: literalField(object, "type", "snapshot"),
        revision: nonNegativeBigIntField(object, "revision"),
        ingestedPackets: nonNegativeInteger(numberField(object, "ingestedPackets"), "ingestedPackets"),
        ingestedCommands: nonNegativeInteger(numberField(object, "ingestedCommands"), "ingestedCommands"),
        routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
        scheduledAnnounces: nonNegativeInteger(numberField(object, "scheduledAnnounces"), "scheduledAnnounces"),
        interfaces: interfacesRaw.map(parseInterfaceSnapshot),
        activeLinkCount: optionalNumber(object, "activeLinkCount", (value) => nonNegativeInteger(value, "activeLinkCount")) ?? 0,
        routeSnapshots: routeSnapshotsRaw.map(parseRouteSnapshot),
        destinationIdentities: destinationIdentitiesRaw.map(parseDestinationIdentitySnapshot),
    };
}
function parseInterfaceSnapshot(raw) {
    const object = record(raw, "InterfaceSnapshot");
    const snapshot = {
        id: interfaceId(bytesField(object, "id")),
        kind: stringField(object, "kind"),
        routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
        links: nonNegativeInteger(numberField(object, "links"), "links"),
        transportedLinks: optionalNumber(object, "transportedLinks", (value) => nonNegativeInteger(value, "transportedLinks")) ?? 0,
    };
    const bitrate = optionalNumber(object, "bitrateBps", bitrateBps);
    if (bitrate !== undefined) {
        snapshot.bitrateBps = bitrate;
    }
    const mtu = optionalNumber(object, "hardwareMtu", hardwareMtu);
    if (mtu !== undefined) {
        snapshot.hardwareMtu = mtu;
    }
    return snapshot;
}
function parseRouteSnapshot(raw) {
    const object = record(raw, "RouteSnapshot");
    const viaIdentity = optionalBytesField(object, "viaIdentity");
    return {
        destination: destinationHash(bytesField(object, "destination")),
        hops: nonNegativeInteger(numberField(object, "hops"), "hops"),
        ...(viaIdentity === undefined
            ? {}
            : { viaIdentity: identityHash(viaIdentity) }),
        interfaceId: interfaceId(bytesField(object, "interfaceId")),
        learnedAtMillis: nonNegativeInteger(numberField(object, "learnedAtMillis"), "learnedAtMillis"),
        lastRouteActivityAtMillis: nonNegativeInteger(numberField(object, "lastRouteActivityAtMillis"), "lastRouteActivityAtMillis"),
        expiresAtMillis: nonNegativeInteger(numberField(object, "expiresAtMillis"), "expiresAtMillis"),
    };
}
function parseDestinationIdentitySnapshot(raw) {
    const object = record(raw, "DestinationIdentitySnapshot");
    return {
        destination: destinationHash(bytesField(object, "destination")),
        identity: identityHash(bytesField(object, "identity")),
    };
}

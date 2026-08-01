import { DESTINATION_HASH_LENGTH, IDENTITY_HASH_LENGTH, IDENTITY_SECRET_LENGTH, INTERFACE_ID_LENGTH, LINK_ID_LENGTH, PACKET_HASH_LENGTH, REQUEST_ID_LENGTH, REQUEST_PATH_HASH_LENGTH, RESOURCE_HASH_LENGTH, } from "./contract.generated.js";
export * from "./contract.generated.js";
export class PrnsValidationError extends Error {
    code;
    constructor(code, message) {
        super(message);
        this.name = "PrnsValidationError";
        this.code = code;
    }
}
export function contractValue(field, value, guard) {
    if (!guard(value)) {
        throw new PrnsValidationError("InvalidEnum", `${field} contains an unknown host contract value`);
    }
    return value;
}
export function destinationHash(bytes) {
    return fixedBytes("destination hash", bytes, DESTINATION_HASH_LENGTH);
}
export function identityHash(bytes) {
    return fixedBytes("identity hash", bytes, IDENTITY_HASH_LENGTH);
}
export function interfaceId(bytes) {
    return fixedBytes("interface ID", bytes, INTERFACE_ID_LENGTH);
}
export function linkId(bytes) {
    return fixedBytes("link ID", bytes, LINK_ID_LENGTH);
}
export function packetHash(bytes) {
    return fixedBytes("packet hash", bytes, PACKET_HASH_LENGTH);
}
export function requestId(bytes) {
    return fixedBytes("request ID", bytes, REQUEST_ID_LENGTH);
}
export function requestPathHash(bytes) {
    return fixedBytes("request path hash", bytes, REQUEST_PATH_HASH_LENGTH);
}
export function resourceHash(bytes) {
    return fixedBytes("resource hash", bytes, RESOURCE_HASH_LENGTH);
}
export function identitySecret(bytes) {
    return fixedBytes("identity secret", bytes, IDENTITY_SECRET_LENGTH);
}
function fixedBytes(label, bytes, length) {
    if (!(bytes instanceof Uint8Array) || bytes.length !== length) {
        throw new PrnsValidationError("InvalidBytes", `${label} must contain exactly ${length} bytes`);
    }
    return bytes.slice();
}

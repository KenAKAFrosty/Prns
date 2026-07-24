export const HOST_CONTRACT_ABI = 1;
export const PRODUCT_VERSION = "0.2.8";
export const DESTINATION_HASH_LENGTH = 16;
export const IDENTITY_HASH_LENGTH = 16;
export const INTERFACE_ID_LENGTH = 8;
export const LINK_ID_LENGTH = 16;
export const REQUEST_ID_LENGTH = 16;
export const REQUEST_PATH_HASH_LENGTH = 16;
export const RESOURCE_HASH_LENGTH = 32;
export const IDENTITY_SECRET_LENGTH = 64;
export class PrnsValidationError extends Error {
    code;
    constructor(code, message) {
        super(message);
        this.name = "PrnsValidationError";
        this.code = code;
    }
}
export function balancedLimits() {
    return {
        pendingCommands: 256,
        applicationEvents: 1_024,
        retainedEventBytes: 8 * 1_024 * 1_024,
        diagnostics: 1_024,
    };
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

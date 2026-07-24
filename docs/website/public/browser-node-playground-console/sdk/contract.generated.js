export const HOST_CONTRACT_ABI = 1;
export const PRODUCT_VERSION = "0.2.8";
export const DESTINATION_HASH_LENGTH = 16;
export const IDENTITY_HASH_LENGTH = 16;
export const INTERFACE_ID_LENGTH = 8;
export const LINK_ID_LENGTH = 16;
export const PACKET_HASH_LENGTH = 32;
export const REQUEST_ID_LENGTH = 16;
export const REQUEST_PATH_HASH_LENGTH = 16;
export const RESOURCE_HASH_LENGTH = 32;
export const IDENTITY_SECRET_LENGTH = 64;
export function balancedLimits() {
    return {
        pendingCommands: 256,
        applicationEvents: 1024,
        retainedEventBytes: 8388608,
        diagnostics: 1024,
    };
}

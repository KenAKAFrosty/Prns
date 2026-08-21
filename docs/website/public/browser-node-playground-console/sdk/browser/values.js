import { Tag } from "../casework.js";
export const MIN_ENTROPY_BYTES = 128;
export const BLE_IDENTITY_LENGTH = 16;
export class PrnsValidationError extends Error {
    code;
    constructor(code, message) {
        super(message);
        this.name = "PrnsValidationError";
        this.code = code;
    }
}
export function identitySecretKey(bytes, expectedLength) {
    return exactBytes(bytes, expectedLength, "IdentitySecretKey");
}
export function bleIdentity(bytes) {
    return bytes.length === BLE_IDENTITY_LENGTH
        ? Tag("ValidBleIdentity", copyBytes(bytes))
        : Tag("InvalidBleIdentity", { actualLength: bytes.length });
}
export function channelTag(bytes) {
    return nonEmptyBytes(bytes, "ChannelTag");
}
export function packetFrame(bytes) {
    return nonEmptyBytes(bytes, "PacketFrame");
}
export function entropyBytes(bytes) {
    if (bytes.length < MIN_ENTROPY_BYTES) {
        throw new PrnsValidationError("invalid-length", `EntropyBytes requires at least ${MIN_ENTROPY_BYTES} bytes`);
    }
    return copyBytes(bytes);
}
export function appData(bytes = new Uint8Array()) {
    return copyBytes(bytes);
}
export function appName(value) {
    return dottedComponent(value, "AppName");
}
export function aspect(value) {
    return dottedComponent(value, "Aspect");
}
export function bitrateBps(value) {
    return positiveInteger(value, "BitrateBps");
}
export function hardwareMtu(value) {
    return positiveInteger(value, "HardwareMtu");
}
export function hopCount(value) {
    if (!Number.isInteger(value) || value < 0 || value > 255) {
        throw new PrnsValidationError("invalid-number", "HopCount must be an integer from 0 through 255");
    }
    return value;
}
export function nowMillis(value = Date.now()) {
    if (!Number.isSafeInteger(value) || value < 0) {
        throw new PrnsValidationError("invalid-number", "InstantMillis must be a non-negative safe integer");
    }
    return value;
}
export function commandId(value) {
    if (value < 0n) {
        throw new PrnsValidationError("invalid-number", "CommandId must be non-negative");
    }
    return value;
}
export function copyBytes(bytes) {
    return new Uint8Array(bytes);
}
export function positiveInteger(value, name) {
    if (!Number.isSafeInteger(value) || value <= 0) {
        throw new PrnsValidationError("invalid-number", `${name} must be a positive safe integer`);
    }
    return value;
}
export function nonNegativeInteger(value, name) {
    if (!Number.isSafeInteger(value) || value < 0) {
        throw new PrnsValidationError("invalid-number", `${name} must be a non-negative safe integer`);
    }
    return value;
}
function exactBytes(bytes, expectedLength, name) {
    if (bytes.length !== expectedLength) {
        throw new PrnsValidationError("invalid-length", `${name} must be ${expectedLength} bytes`);
    }
    return copyBytes(bytes);
}
function nonEmptyBytes(bytes, name) {
    if (bytes.length === 0) {
        throw new PrnsValidationError("empty-bytes", `${name} must not be empty`);
    }
    return copyBytes(bytes);
}
function dottedComponent(value, name) {
    if (value.length === 0) {
        throw new PrnsValidationError("empty-string", `${name} must not be empty`);
    }
    if (value.includes(".")) {
        throw new PrnsValidationError("invalid-component", `${name} must not contain dots`);
    }
    return value;
}

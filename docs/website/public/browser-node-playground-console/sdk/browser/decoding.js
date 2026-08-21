import { PrnsValidationError } from "./values.js";
export function field(object, key) {
    if (!(key in object)) {
        throw new PrnsValidationError("invalid-component", `missing field ${key}`);
    }
    return object[key];
}
export function stringField(object, key) {
    const value = field(object, key);
    if (typeof value !== "string") {
        throw new PrnsValidationError("invalid-component", `${key} must be a string`);
    }
    return value;
}
export function literalField(object, key, expected) {
    const value = stringField(object, key);
    if (value !== expected) {
        throw new PrnsValidationError("invalid-component", `${key} must be ${expected}`);
    }
    return expected;
}
export function numberField(object, key) {
    const value = field(object, key);
    if (typeof value !== "number") {
        throw new PrnsValidationError("invalid-component", `${key} must be a number`);
    }
    return value;
}
export function nonNegativeBigIntField(object, key) {
    const value = field(object, key);
    if (typeof value !== "bigint" || value < 0n) {
        throw new PrnsValidationError("invalid-component", `${key} must be a non-negative bigint`);
    }
    return value;
}
export function optionalNumber(object, key, parse) {
    if (!(key in object)) {
        return undefined;
    }
    return parse(numberField(object, key));
}
export function optionalBytesField(object, key) {
    return key in object ? bytesField(object, key) : undefined;
}
export function optionalArrayField(object, key) {
    if (!(key in object)) {
        return [];
    }
    const value = field(object, key);
    if (!Array.isArray(value)) {
        throw new PrnsValidationError("invalid-component", `${key} must be an array`);
    }
    return value;
}
export function bigintField(object, key) {
    const value = field(object, key);
    if (typeof value === "bigint") {
        return value;
    }
    if (typeof value === "number" && Number.isSafeInteger(value)) {
        return BigInt(value);
    }
    throw new PrnsValidationError("invalid-component", `${key} must be a bigint or safe integer`);
}
export function bytesField(object, key) {
    const value = field(object, key);
    if (!(value instanceof Uint8Array)) {
        throw new PrnsValidationError("invalid-component", `${key} must be a Uint8Array`);
    }
    return value;
}
export function record(value, name) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new PrnsValidationError("invalid-component", `${name} must be an object`);
    }
    return value;
}

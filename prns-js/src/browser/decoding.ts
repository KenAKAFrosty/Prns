import { PrnsValidationError } from "./values.js";

export function field(
  object: Record<string, unknown>,
  key: string,
): unknown {
  if (!(key in object)) {
    throw new PrnsValidationError(
      "invalid-component",
      `missing field ${key}`,
    );
  }
  return object[key];
}

export function stringField(
  object: Record<string, unknown>,
  key: string,
): string {
  const value = field(object, key);
  if (typeof value !== "string") {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a string`,
    );
  }
  return value;
}

export function literalField<T extends string>(
  object: Record<string, unknown>,
  key: string,
  expected: T,
): T {
  const value = stringField(object, key);
  if (value !== expected) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be ${expected}`,
    );
  }
  return expected;
}

export function numberField(
  object: Record<string, unknown>,
  key: string,
): number {
  const value = field(object, key);
  if (typeof value !== "number") {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a number`,
    );
  }
  return value;
}

export function nonNegativeBigIntField(
  object: Record<string, unknown>,
  key: string,
): bigint {
  const value = field(object, key);
  if (typeof value !== "bigint" || value < 0n) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a non-negative bigint`,
    );
  }
  return value;
}

export function optionalNumber<T>(
  object: Record<string, unknown>,
  key: string,
  parse: (value: number) => T,
): T | undefined {
  if (!(key in object)) {
    return undefined;
  }
  return parse(numberField(object, key));
}

export function optionalBytesField(
  object: Record<string, unknown>,
  key: string,
): Uint8Array | undefined {
  return key in object ? bytesField(object, key) : undefined;
}

export function optionalArrayField(
  object: Record<string, unknown>,
  key: string,
): unknown[] {
  if (!(key in object)) {
    return [];
  }
  const value = field(object, key);
  if (!Array.isArray(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be an array`,
    );
  }
  return value;
}

export function bigintField(
  object: Record<string, unknown>,
  key: string,
): bigint {
  const value = field(object, key);
  if (typeof value === "bigint") {
    return value;
  }
  if (typeof value === "number" && Number.isSafeInteger(value)) {
    return BigInt(value);
  }
  throw new PrnsValidationError(
    "invalid-component",
    `${key} must be a bigint or safe integer`,
  );
}

export function bytesField(
  object: Record<string, unknown>,
  key: string,
): Uint8Array {
  const value = field(object, key);
  if (!(value instanceof Uint8Array)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a Uint8Array`,
    );
  }
  return value;
}

export function record(
  value: unknown,
  name: string,
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${name} must be an object`,
    );
  }
  return value as Record<string, unknown>;
}

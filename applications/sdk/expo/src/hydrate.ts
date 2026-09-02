import {
  destinationHash,
  identityHash,
  type DestinationHash,
  type IdentityHash,
} from "personal-rns/contract";

type IdentityByteKey = "controllerIdentityFingerprint" | "targetIdentityFingerprint";
type DestinationByteKey = "destination" | "endpoint";
type OpaqueByteKey = "publicAppData";
type U64Key = "expiresAtMillis" | "observedAtMillis" | "revision" | "rttMillis" | "startedAtMillis";

type HydratedIdentity<Value> = null extends Value ? IdentityHash | null : IdentityHash;

type HydratedProperty<Key extends PropertyKey, Value> = Key extends IdentityByteKey
  ? HydratedIdentity<Value>
  : Key extends DestinationByteKey
    ? DestinationHash
    : Key extends OpaqueByteKey
      ? Uint8Array
      : Key extends U64Key
        ? bigint
        : Hydrated<Value>;

export type Hydrated<Value> = Value extends readonly (infer Item)[]
  ? readonly Hydrated<Item>[]
  : Value extends object
    ? { readonly [Key in keyof Value]: HydratedProperty<Key, Value[Key]> }
    : Value;

const identityByteKeys = new Set<string>([
  "controllerIdentityFingerprint",
  "targetIdentityFingerprint",
]);
const destinationByteKeys = new Set<string>(["destination", "endpoint"]);
const u64Keys = new Set<string>([
  "expiresAtMillis",
  "observedAtMillis",
  "revision",
  "rttMillis",
  "startedAtMillis",
]);
const maximumU64 = 18_446_744_073_709_551_615n;
const canonicalUnsignedDecimal = /^(0|[1-9][0-9]*)$/;

export class NativePayloadError extends Error {
  readonly path: string;

  constructor(path: string, detail: string, options?: ErrorOptions) {
    super(`Invalid native payload at ${path}: ${detail}`, options);
    this.name = "NativePayloadError";
    this.path = path;
  }
}

export function hydrateGenerated<Value>(value: Value): Hydrated<Value> {
  return hydrateValue(value, "$", undefined) as Hydrated<Value>;
}

function hydrateValue(value: unknown, path: string, key: string | undefined): unknown {
  if (key !== undefined && u64Keys.has(key)) {
    return hydrateU64(value, path);
  }
  if (key !== undefined && identityByteKeys.has(key)) {
    if (value === null) {
      return null;
    }
    const bytes = hydrateBytes(value, path);
    try {
      return identityHash(bytes);
    } catch (cause) {
      throw new NativePayloadError(path, "expected a Prns identity hash", { cause });
    }
  }
  if (key !== undefined && destinationByteKeys.has(key)) {
    const bytes = hydrateBytes(value, path);
    try {
      return destinationHash(bytes);
    } catch (cause) {
      throw new NativePayloadError(path, "expected a Prns destination hash", { cause });
    }
  }
  if (key === "publicAppData") {
    return hydrateBytes(value, path);
  }
  if (Array.isArray(value)) {
    return value.map((item, index) => hydrateValue(item, `${path}[${index}]`, undefined));
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([property, propertyValue]) => [
        property,
        hydrateValue(propertyValue, `${path}.${property}`, property),
      ]),
    );
  }
  return value;
}

function hydrateBytes(value: unknown, path: string): Uint8Array {
  if (
    !Array.isArray(value) ||
    !value.every(
      (byte) => typeof byte === "number" && Number.isInteger(byte) && byte >= 0 && byte <= 255,
    )
  ) {
    throw new NativePayloadError(path, "expected an array of byte values");
  }
  return Uint8Array.from(value);
}

function hydrateU64(value: unknown, path: string): bigint {
  if (typeof value !== "string" || !canonicalUnsignedDecimal.test(value)) {
    throw new NativePayloadError(path, "expected a canonical unsigned decimal string");
  }
  const parsed = BigInt(value);
  if (parsed > maximumU64) {
    throw new NativePayloadError(path, "unsigned decimal exceeds u64::MAX");
  }
  return parsed;
}

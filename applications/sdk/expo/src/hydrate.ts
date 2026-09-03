import {
  destinationHash,
  identityHash,
  type DestinationHash,
  type HostSnapshot,
  type IdentityHash,
  interfaceId,
  type InterfaceId,
} from "personal-rns/contract";

type IdentityByteKey =
  | "controllerIdentityFingerprint"
  | "attempted"
  | "existing"
  | "identity"
  | "identityHash"
  | "targetIdentityFingerprint"
  | "viaIdentity";
type DestinationByteKey = "destination" | "endpoint" | "peer" | "source";
type InterfaceByteKey = "interfaceId";
type OpaqueByteKey = "bytes" | "messageId" | "publicAppData";
type U64Key =
  | "before"
  | "expiresAtMillis"
  | "inboundOverflowCount"
  | "lastObservedAgeMillis"
  | "localRecordId"
  | "observedAtMillis"
  | "requiredStampCost"
  | "revision"
  | "rttMillis"
  | "rxBytes"
  | "startedAtMillis"
  | "timestamp"
  | "txBytes";

type HydratedIdentity<Value> = null extends Value ? IdentityHash | null : IdentityHash;
type HydratedDestination<Value> = null extends Value ? DestinationHash | null : DestinationHash;
type HydratedU64<Value> = null extends Value ? bigint | null : bigint;

type HydratedProperty<Key extends PropertyKey, Value> = Key extends IdentityByteKey
  ? HydratedIdentity<Value>
  : Key extends DestinationByteKey
    ? HydratedDestination<Value>
    : Key extends InterfaceByteKey
      ? InterfaceId
      : Key extends OpaqueByteKey
        ? null extends Value
          ? Uint8Array | null
          : Uint8Array
        : Key extends U64Key
          ? HydratedU64<Value>
          : Hydrated<Value>;

export type Hydrated<Value> = Value extends HostSnapshot
  ? HostSnapshot
  : Value extends readonly (infer Item)[]
    ? readonly Hydrated<Item>[]
    : Value extends object
      ? { readonly [Key in keyof Value]: HydratedProperty<Key, Value[Key]> }
      : Value;

const identityByteKeys = new Set<string>([
  "controllerIdentityFingerprint",
  "attempted",
  "existing",
  "identity",
  "identityHash",
  "targetIdentityFingerprint",
  "viaIdentity",
]);
const destinationByteKeys = new Set<string>(["destination", "endpoint", "peer", "source"]);
const u64Keys = new Set<string>([
  "before",
  "inboundOverflowCount",
  "lastObservedAgeMillis",
  "localRecordId",
  "observedAtMillis",
  "requiredStampCost",
  "revision",
  "rttMillis",
  "rxBytes",
  "startedAtMillis",
  "timestamp",
  "txBytes",
]);
const safeUintKeys = new Set<string>([
  "expiresAtMillis",
  "lastRouteActivityAtMillis",
  "learnedAtMillis",
  "rxBps",
  "txBps",
  "uptimeMillis",
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
  if (key === "expiresAtMillis" && !isHostRoutePath(path)) {
    return hydrateU64(value, path);
  }
  if (key !== undefined && u64Keys.has(key)) {
    if (value === null) {
      return null;
    }
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
  if (key === "messageId") {
    const bytes = hydrateBytes(value, path);
    if (bytes.length !== 32) {
      throw new NativePayloadError(path, "expected exactly 32 message ID bytes");
    }
    return bytes;
  }
  if (key !== undefined && destinationByteKeys.has(key)) {
    if (value === null) {
      return null;
    }
    const bytes = hydrateBytes(value, path);
    try {
      return destinationHash(bytes);
    } catch (cause) {
      throw new NativePayloadError(path, "expected a Prns destination hash", { cause });
    }
  }
  if (key === "interfaceId") {
    const bytes = hydrateBytes(value, path);
    try {
      return interfaceId(bytes);
    } catch (cause) {
      throw new NativePayloadError(path, "expected a Prns interface ID", { cause });
    }
  }
  if (key === "bytes" || key === "publicAppData") {
    if (value === null) {
      return null;
    }
    return hydrateBytes(value, path);
  }
  if (key !== undefined && safeUintKeys.has(key)) {
    return hydrateSafeUint(value, path);
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

function isHostRoutePath(path: string): boolean {
  return /\.routes\[[0-9]+\]\.expiresAtMillis$/.test(path);
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

function hydrateSafeUint(value: unknown, path: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new NativePayloadError(path, "expected a non-negative safe integer");
  }
  return value;
}

import {
  DESTINATION_HASH_LENGTH,
  IDENTITY_HASH_LENGTH,
  IDENTITY_SECRET_LENGTH,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  REQUEST_ID_LENGTH,
  REQUEST_PATH_HASH_LENGTH,
  RESOURCE_HASH_LENGTH,
} from "./contract.generated.js";
import type {
  CapabilityName,
  DestinationConfig,
  DestinationHash,
  IdentityConfig,
  IdentityHash,
  IdentitySecret,
  HostRoleName,
  InterfaceId,
  LinkId,
  PrnsLimits,
  RequestId,
  RequestPathHash,
  ResourceHash,
} from "./contract.generated.js";
import type { Tag } from "./casework.js";

export * from "./contract.generated.js";

export type PrnsValidationCode =
  | "EmptyString"
  | "InvalidBytes"
  | "InvalidLimit"
  | "InvalidNumber"
  | "MissingDestinationAspect";

export class PrnsValidationError extends Error {
  readonly code: PrnsValidationCode;

  constructor(code: PrnsValidationCode, message: string) {
    super(message);
    this.name = "PrnsValidationError";
    this.code = code;
  }
}

export type PrnsCreateOptions = {
  readonly identity: IdentityConfig;
  readonly role: HostRoleName;
  readonly destinations?: readonly DestinationConfig[];
  readonly limits?: PrnsLimits;
};

export type LifecycleState =
  | Tag<"Starting">
  | Tag<"Running">
  | Tag<"Stopping">
  | Tag<"Stopped", { readonly reason: "Requested" | "BackendExited" }>
  | Tag<
      "Failed",
      | {
          readonly cause: "EventBackpressureExceeded";
          readonly limits: PrnsLimits;
          readonly rejectedEventBytes: number;
        }
      | {
          readonly cause: "BackendFailed" | "ContractViolated";
          readonly detail: string;
        }
    >;

export type BackendCapabilities =
  | Tag<
      "Native",
      {
        readonly available: ReadonlySet<CapabilityName>;
      }
    >
  | Tag<
      "Browser",
      {
        readonly available: ReadonlySet<CapabilityName>;
      }
    >;

export type ContractMismatch = Tag<
  "ContractMismatch",
  {
    readonly requiredAbi: number;
    readonly actualAbi: number;
    readonly requiredProductVersion: string;
    readonly actualProductVersion: string;
  }
>;

export type CapabilityUnavailable = Tag<
  "CapabilityUnavailable",
  { readonly capability: CapabilityName }
>;

export type BackendStartFailed = Tag<
  "BackendStartFailed",
  { readonly detail: string; readonly code?: string }
>;

export function destinationHash(bytes: Uint8Array): DestinationHash {
  return fixedBytes("destination hash", bytes, DESTINATION_HASH_LENGTH);
}

export function identityHash(bytes: Uint8Array): IdentityHash {
  return fixedBytes("identity hash", bytes, IDENTITY_HASH_LENGTH);
}

export function interfaceId(bytes: Uint8Array): InterfaceId {
  return fixedBytes("interface ID", bytes, INTERFACE_ID_LENGTH);
}

export function linkId(bytes: Uint8Array): LinkId {
  return fixedBytes("link ID", bytes, LINK_ID_LENGTH);
}

export function requestId(bytes: Uint8Array): RequestId {
  return fixedBytes("request ID", bytes, REQUEST_ID_LENGTH);
}

export function requestPathHash(bytes: Uint8Array): RequestPathHash {
  return fixedBytes("request path hash", bytes, REQUEST_PATH_HASH_LENGTH);
}

export function resourceHash(bytes: Uint8Array): ResourceHash {
  return fixedBytes("resource hash", bytes, RESOURCE_HASH_LENGTH);
}

export function identitySecret(bytes: Uint8Array): IdentitySecret {
  return fixedBytes("identity secret", bytes, IDENTITY_SECRET_LENGTH);
}

function fixedBytes<Value extends Uint8Array>(
  label: string,
  bytes: Uint8Array,
  length: number,
): Value {
  if (!(bytes instanceof Uint8Array) || bytes.length !== length) {
    throw new PrnsValidationError(
      "InvalidBytes",
      `${label} must contain exactly ${length} bytes`,
    );
  }
  return bytes.slice() as Value;
}

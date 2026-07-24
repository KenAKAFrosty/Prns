import type { Tag } from "./casework.js";

declare const brand: unique symbol;

type Brand<Name extends string> = { readonly [brand]: Name };
type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;

export const HOST_CONTRACT_ABI = 1;
export const PRODUCT_VERSION = "0.2.8";
export const DESTINATION_HASH_LENGTH = 16;
export const IDENTITY_HASH_LENGTH = 16;
export const INTERFACE_ID_LENGTH = 8;
export const LINK_ID_LENGTH = 16;
export const REQUEST_ID_LENGTH = 16;
export const REQUEST_PATH_HASH_LENGTH = 16;
export const IDENTITY_SECRET_LENGTH = 64;

export type DestinationHash = BrandedBytes<"DestinationHash">;
export type IdentityHash = BrandedBytes<"IdentityHash">;
export type InterfaceId = BrandedBytes<"InterfaceId">;
export type LinkId = BrandedBytes<"LinkId">;
export type RequestId = BrandedBytes<"RequestId">;
export type RequestPathHash = BrandedBytes<"RequestPathHash">;
export type IdentitySecret = BrandedBytes<"IdentitySecret">;

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

export type PrnsLimits = {
  readonly pendingCommands: number;
  readonly applicationEvents: number;
  readonly retainedEventBytes: number;
  readonly diagnostics: number;
};

export function balancedLimits(): PrnsLimits {
  return {
    pendingCommands: 256,
    applicationEvents: 1_024,
    retainedEventBytes: 8 * 1_024 * 1_024,
    diagnostics: 1_024,
  };
}

export type IdentityConfig =
  | Tag<"Existing", { readonly secret: IdentitySecret }>
  | Tag<"GenerateEphemeral">
  | Tag<"LoadOrCreate", { readonly path: string }>;

export type DestinationName = {
  readonly appName: string;
  readonly aspects: readonly string[];
};

export type DestinationConfig =
  | Tag<"Plain", { readonly name: DestinationName }>
  | Tag<
      "Single",
      {
        readonly name: DestinationName;
        readonly identity?: IdentityConfig;
        readonly announceAppData?: Uint8Array;
      }
    >;

export type PrnsCreateOptions = {
  readonly identity: IdentityConfig;
  readonly destinations?: readonly DestinationConfig[];
  readonly limits?: PrnsLimits;
  readonly transport?: boolean;
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

export type CapabilityName =
  | "Loopback"
  | "TcpClient"
  | "TcpServer"
  | "Udp"
  | "Serial"
  | "Usb"
  | "Bluetooth"
  | "Wifi"
  | "WebSocket"
  | "BrowserRendezvous"
  | "I2p"
  | "Weave";

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

export type ResourceStream = AsyncIterable<Uint8Array> & {
  readonly totalBytes: number;
};

export type ApplicationEvent =
  | Tag<
      "SingleDelivery",
      {
        readonly destination: DestinationHash;
        readonly sourceInterface: InterfaceId;
        readonly plaintext: Uint8Array;
      }
    >
  | Tag<
      "Request",
      {
        readonly destination: DestinationHash;
        readonly linkId: LinkId;
        readonly requestId: RequestId;
        readonly requester?: IdentityHash;
        readonly pathHash: RequestPathHash;
        readonly rttMillis: number;
        readonly data: Uint8Array;
        readonly responseToken: unknown;
      }
    >
  | Tag<
      "Response",
      {
        readonly linkId: LinkId;
        readonly requestId: RequestId;
        readonly data: Uint8Array;
      }
    >
  | Tag<
      "ResourceAvailable",
      {
        readonly linkId: LinkId;
        readonly hash: Uint8Array;
        readonly metadata?: Uint8Array;
        readonly resource: ResourceStream;
      }
    >
  | Tag<
      "ChannelMessage",
      {
        readonly linkId: LinkId;
        readonly messageType: string;
        readonly data: Uint8Array;
      }
    >;

export type DiagnosticEvent =
  | Tag<
      "AnnounceHeard",
      {
        readonly destination: DestinationHash;
        readonly hops: number;
        readonly sourceInterface: InterfaceId;
      }
    >
  | Tag<
      "LinkEstablished",
      { readonly linkId: LinkId; readonly rttMillis: number }
    >
  | Tag<
      "PeerIdentified",
      { readonly linkId: LinkId; readonly identity: IdentityHash }
    >
  | Tag<
      "LinkClosed",
      {
        readonly linkId: LinkId;
        readonly reason: "Timeout" | "PeerClosed" | "MalformedRtt";
      }
    >
  | Tag<
      "BackendDiagnostic",
      { readonly kind: string; readonly detail: string }
    >
  | Tag<"DiagnosticsDropped", { readonly count: bigint }>;

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

export function identitySecret(bytes: Uint8Array): IdentitySecret {
  return fixedBytes("identity secret", bytes, IDENTITY_SECRET_LENGTH);
}

function fixedBytes<Name extends string, Value extends Uint8Array & Brand<Name>>(
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

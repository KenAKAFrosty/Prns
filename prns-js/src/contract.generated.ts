import type { Tag } from "./casework.js";
import type { StreamClaim } from "./async_lanes.js";

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
export const RESOURCE_HASH_LENGTH = 32;
export const IDENTITY_SECRET_LENGTH = 64;

export type DestinationHash = BrandedBytes<"DestinationHash">;
export type IdentityHash = BrandedBytes<"IdentityHash">;
export type InterfaceId = BrandedBytes<"InterfaceId">;
export type LinkId = BrandedBytes<"LinkId">;
export type RequestId = BrandedBytes<"RequestId">;
export type RequestPathHash = BrandedBytes<"RequestPathHash">;
export type ResourceHash = BrandedBytes<"ResourceHash">;
export type IdentitySecret = BrandedBytes<"IdentitySecret">;

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

export type LinkClosedReason =
  | "Timeout"
  | "PeerClosed"
  | "MalformedRtt";

export type PrnsLimits = {
  readonly pendingCommands: number;
  readonly applicationEvents: number;
  readonly retainedEventBytes: number;
  readonly diagnostics: number;
};

export function balancedLimits(): PrnsLimits {
  return {
    pendingCommands: 256,
    applicationEvents: 1024,
    retainedEventBytes: 8388608,
    diagnostics: 1024,
  };
}

export type DestinationName = {
  readonly appName: string;
  readonly aspects: readonly string[];
};

export type ResourceStream = {
  readonly totalBytes: number;
  claim(): StreamClaim<Uint8Array>;
};

export type IdentityConfig =
  | Tag<
      "Existing",
      {
        readonly secret: IdentitySecret;
      }
    >
  | Tag<"GenerateEphemeral">
  | Tag<
      "LoadOrCreate",
      {
        readonly path: string;
      }
    >;

export type DestinationConfig =
  | Tag<
      "Plain",
      {
        readonly name: DestinationName;
      }
    >
  | Tag<
      "Single",
      {
        readonly name: DestinationName;
        readonly identity?: IdentityConfig;
        readonly announceAppData?: Uint8Array;
      }
    >;

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
      "ResponseSegment",
      {
        readonly linkId: LinkId;
        readonly requestId: RequestId;
        readonly segmentIndex: number;
        readonly totalSegments: number;
        readonly data: Uint8Array;
      }
    >
  | Tag<
      "ResourceAvailable",
      {
        readonly linkId: LinkId;
        readonly hash: ResourceHash;
        readonly metadata?: Uint8Array;
        readonly resource: ResourceStream;
      }
    >
  | Tag<
      "ResourceSegment",
      {
        readonly linkId: LinkId;
        readonly originalHash: ResourceHash;
        readonly segmentIndex: number;
        readonly totalSegments: number;
        readonly metadata?: Uint8Array;
        readonly data: Uint8Array;
      }
    >
  | Tag<
      "ResourceNeedsDecompression",
      {
        readonly linkId: LinkId;
        readonly hash: ResourceHash;
        readonly stream: Uint8Array;
        readonly uncompressedDataBytes: number;
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
      {
        readonly linkId: LinkId;
        readonly rttMillis: number;
      }
    >
  | Tag<
      "PeerIdentified",
      {
        readonly linkId: LinkId;
        readonly identity: IdentityHash;
      }
    >
  | Tag<
      "LinkClosed",
      {
        readonly linkId: LinkId;
        readonly reason: LinkClosedReason;
      }
    >
  | Tag<
      "LinkInterfaceMismatch",
      {
        readonly linkId: LinkId;
        readonly attachedInterface: InterfaceId;
        readonly arrivedOn: InterfaceId;
      }
    >
  | Tag<
      "ResourceAssembled",
      {
        readonly linkId: LinkId;
        readonly originalHash: ResourceHash;
        readonly totalSizeBytes: number;
      }
    >
  | Tag<
      "ResourceFailed",
      {
        readonly linkId: LinkId;
        readonly hash: ResourceHash;
        readonly cause: string;
      }
    >
  | Tag<
      "ResourceSendProgress",
      {
        readonly linkId: LinkId;
        readonly transferredBytes: number;
        readonly totalBytes: number;
        readonly physicalTransferredBytes: number;
        readonly segmentIndex: number;
        readonly totalSegments: number;
      }
    >
  | Tag<
      "SelfRatchetRotated",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "AnnounceHeldDropped",
      {
        readonly destination: DestinationHash;
        readonly sourceInterface: InterfaceId;
        readonly cause: string;
      }
    >
  | Tag<
      "Delivered",
      {
        readonly detail: string;
      }
    >
  | Tag<
      "RouteExpired",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "RouteEvicted",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "RouteInterfaceGone",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "RouteDropped",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "BackendDiagnostic",
      {
        readonly kind: string;
        readonly detail: string;
      }
    >
  | Tag<
      "DiagnosticsDropped",
      {
        readonly count: bigint;
      }
    >;

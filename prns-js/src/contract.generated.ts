import type { Tag } from "./casework.js";
import type { StreamClaim } from "./async_lanes.js";

declare const brand: unique symbol;

type Brand<Name extends string> = { readonly [brand]: Name };
type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;

export const HOST_CONTRACT_ABI = 1;
export const PRODUCT_VERSION = "0.3.1";
export const DESTINATION_HASH_LENGTH = 16;
export const IDENTITY_HASH_LENGTH = 16;
export const INTERFACE_ID_LENGTH = 8;
export const LINK_ID_LENGTH = 16;
export const PACKET_HASH_LENGTH = 32;
export const REQUEST_ID_LENGTH = 16;
export const REQUEST_PATH_HASH_LENGTH = 16;
export const RESOURCE_HASH_LENGTH = 32;
export const IDENTITY_SECRET_LENGTH = 64;

export type DestinationHash = BrandedBytes<"DestinationHash">;
export type IdentityHash = BrandedBytes<"IdentityHash">;
export type InterfaceId = BrandedBytes<"InterfaceId">;
export type LinkId = BrandedBytes<"LinkId">;
export type PacketHash = BrandedBytes<"PacketHash">;
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

export type HostRoleName =
  | "Endpoint"
  | "Transport";

export type DeliveryEvidenceKind =
  | "ExplicitProof"
  | "ImplicitProof"
  | "Response";

export type RequestPolicy =
  | "AllowNone"
  | "AllowAll"
  | "AllowList";

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

export type RequestHandlerConfig = {
  readonly path: string;
  readonly policy: RequestPolicy;
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

export type DestinationIdentityConfig =
  | Tag<"HostIdentity">
  | Tag<
      "DedicatedIdentity",
      {
        readonly identity: IdentityConfig;
      }
    >;

export type Bitrate =
  | Tag<"Auto">
  | Tag<
      "BitsPerSecond",
      {
        readonly value: number;
      }
    >;

export type ResponseTimeout =
  | Tag<"LinkDefault">
  | Tag<
      "Exact",
      {
        readonly millis: number;
      }
    >;

export type ResourceCompression =
  | Tag<"Auto">
  | Tag<"Never">;

export type ResourceStrategy =
  | Tag<"Refuse">
  | Tag<
      "Accept",
      {
        readonly maximumUncompressedBytes: number;
        readonly acceptCompressed: boolean;
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
        readonly identity: DestinationIdentityConfig;
        readonly announceAppData?: Uint8Array;
        readonly requestHandlers: readonly RequestHandlerConfig[];
      }
    >;

export type HostCommand =
  | Tag<
      "Announce",
      {
        readonly destination: DestinationHash;
        readonly interface?: InterfaceId;
      }
    >
  | Tag<
      "SendSinglePacket",
      {
        readonly destination: DestinationHash;
        readonly payload: Uint8Array;
      }
    >
  | Tag<
      "CloseLink",
      {
        readonly linkId: LinkId;
      }
    >
  | Tag<
      "AttachTcpServer",
      {
        readonly bind: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "AttachTcpClient",
      {
        readonly target: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "AttachUdp",
      {
        readonly local: string;
        readonly peer: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "DetachInterface",
      {
        readonly interface: InterfaceId;
      }
    >
  | Tag<
      "EstablishLink",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "RequestPath",
      {
        readonly destination: DestinationHash;
      }
    >
  | Tag<
      "Identify",
      {
        readonly linkId: LinkId;
        readonly identity: IdentityHash;
      }
    >
  | Tag<
      "SendLinkPacket",
      {
        readonly linkId: LinkId;
        readonly payload: Uint8Array;
      }
    >
  | Tag<
      "Request",
      {
        readonly linkId: LinkId;
        readonly pathHash: RequestPathHash;
        readonly payload: Uint8Array;
        readonly timeout: ResponseTimeout;
      }
    >
  | Tag<
      "Respond",
      {
        readonly linkId: LinkId;
        readonly requestId: RequestId;
        readonly requestRttMillis: number;
        readonly payload: Uint8Array;
      }
    >
  | Tag<
      "SendResource",
      {
        readonly linkId: LinkId;
        readonly payload: Uint8Array;
        readonly packedMetadata?: Uint8Array;
        readonly compression: ResourceCompression;
      }
    >
  | Tag<
      "SetLinkResourceStrategy",
      {
        readonly linkId: LinkId;
        readonly strategy: ResourceStrategy;
      }
    >
  | Tag<
      "SetDestinationResourceStrategy",
      {
        readonly destination: DestinationHash;
        readonly strategy: ResourceStrategy;
      }
    >
  | Tag<
      "SendChannelMessage",
      {
        readonly linkId: LinkId;
        readonly messageType: number;
        readonly payload: Uint8Array;
      }
    >
  | Tag<
      "AllowRequester",
      {
        readonly destination: DestinationHash;
        readonly pathHash: RequestPathHash;
        readonly identity: IdentityHash;
      }
    >;

export type CommandOutcome =
  | Tag<"Announced">
  | Tag<
      "PacketDelivered",
      {
        readonly rttMillis: number;
        readonly evidence: DeliveryEvidenceKind;
        readonly packetHash?: PacketHash;
      }
    >
  | Tag<"LinkCloseQueued">
  | Tag<
      "InterfaceAttached",
      {
        readonly interface: InterfaceId;
      }
    >
  | Tag<
      "InterfaceDetached",
      {
        readonly interface: InterfaceId;
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
      "PathDiscovered",
      {
        readonly hops: number;
      }
    >
  | Tag<"Identified">
  | Tag<
      "ResponseReceived",
      {
        readonly data: Uint8Array;
        readonly rttMillis: number;
      }
    >
  | Tag<
      "ResponseSent",
      {
        readonly rttMillis: number;
      }
    >
  | Tag<"ResourceSent">
  | Tag<"ResourceStrategySet">
  | Tag<"RequesterAllowed">;

export type CommandFailure =
  | Tag<"NodeStopped">
  | Tag<"Busy">
  | Tag<"PayloadTooLarge">
  | Tag<"UnknownDestination">
  | Tag<"NotSingleDestination">
  | Tag<"AnnounceAppDataTooLong">
  | Tag<"UnknownInterface">
  | Tag<"NoRouteToDestination">
  | Tag<"NotDirectlyReachable">
  | Tag<"PacketCulled">
  | Tag<"DeliveryTimedOut">
  | Tag<"InvalidBitrate">
  | Tag<
      "BindFailed",
      {
        readonly detail: string;
      }
    >
  | Tag<
      "WriteFailed",
      {
        readonly detail: string;
      }
    >
  | Tag<"UnsupportedByBackend">
  | Tag<"UnknownLink">
  | Tag<"LinkNotActive">
  | Tag<"EntropyUnavailable">
  | Tag<"NotLinkInitiator">
  | Tag<"IdentityNotHeld">
  | Tag<"UnknownRequestHandler">
  | Tag<"RequestPolicyNotAllowList">
  | Tag<"RequestAllowListFull">
  | Tag<"LinkBusy">
  | Tag<"ResourceTableFull">
  | Tag<"ResourceMetadataTooLarge">
  | Tag<"ResourceRejectedByPeer">
  | Tag<"ResourceSequencingFailed">
  | Tag<"ResourcePredecessorFailed">
  | Tag<"ChannelWindowFull">
  | Tag<"ChannelUntrackable">
  | Tag<"InvalidChannelMessageType">;

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
        readonly messageType: number;
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

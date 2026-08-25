import type { Tag } from "./casework.js";
import type { StreamClaim } from "./async_lanes.js";

declare const brand: unique symbol;

type Brand<Name extends string> = { readonly [brand]: Name };
type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;

export const HOST_CONTRACT_ABI = 1;
export const HOST_SCHEMA_VERSION = 1;
export const PRODUCT_VERSION = "0.3.7";
export const DESTINATION_HASH_LENGTH = 16;
export const IDENTITY_HASH_LENGTH = 16;
export const INTERFACE_ID_LENGTH = 8;
export const LINK_ID_LENGTH = 16;
export const PACKET_HASH_LENGTH = 32;
export const REQUEST_ID_LENGTH = 16;
export const REQUEST_PATH_HASH_LENGTH = 16;
export const RESOURCE_HASH_LENGTH = 32;
export const IDENTITY_SECRET_LENGTH = 64;
export const SAFE_INT_MIN = -9007199254740991;
export const SAFE_INT_MAX = 9007199254740991;
export const SAFE_UINT_MAX = 9007199254740991;

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
  | "Weave"
  | "SuppliedPipe";

export const CAPABILITY_NAME_VALUES: readonly CapabilityName[] = Object.freeze([
  "Loopback",
  "TcpClient",
  "TcpServer",
  "Udp",
  "Serial",
  "Usb",
  "Bluetooth",
  "Wifi",
  "WebSocket",
  "BrowserRendezvous",
  "I2p",
  "Weave",
  "SuppliedPipe",
]);

export function isCapabilityName(value: unknown): value is CapabilityName {
  return typeof value === "string" && (CAPABILITY_NAME_VALUES as readonly string[]).includes(value);
}

export type LinkClosedReason =
  | "Timeout"
  | "PeerClosed"
  | "MalformedRtt";

export const LINK_CLOSED_REASON_VALUES: readonly LinkClosedReason[] = Object.freeze([
  "Timeout",
  "PeerClosed",
  "MalformedRtt",
]);

export function isLinkClosedReason(value: unknown): value is LinkClosedReason {
  return typeof value === "string" && (LINK_CLOSED_REASON_VALUES as readonly string[]).includes(value);
}

export type HostRoleName =
  | "Endpoint"
  | "Transport";

export const HOST_ROLE_NAME_VALUES: readonly HostRoleName[] = Object.freeze([
  "Endpoint",
  "Transport",
]);

export function isHostRoleName(value: unknown): value is HostRoleName {
  return typeof value === "string" && (HOST_ROLE_NAME_VALUES as readonly string[]).includes(value);
}

export type DeliveryEvidenceKind =
  | "ExplicitProof"
  | "ImplicitProof"
  | "Response";

export const DELIVERY_EVIDENCE_KIND_VALUES: readonly DeliveryEvidenceKind[] = Object.freeze([
  "ExplicitProof",
  "ImplicitProof",
  "Response",
]);

export function isDeliveryEvidenceKind(value: unknown): value is DeliveryEvidenceKind {
  return typeof value === "string" && (DELIVERY_EVIDENCE_KIND_VALUES as readonly string[]).includes(value);
}

export type RequestPolicy =
  | "AllowNone"
  | "AllowAll"
  | "AllowList";

export const REQUEST_POLICY_VALUES: readonly RequestPolicy[] = Object.freeze([
  "AllowNone",
  "AllowAll",
  "AllowList",
]);

export function isRequestPolicy(value: unknown): value is RequestPolicy {
  return typeof value === "string" && (REQUEST_POLICY_VALUES as readonly string[]).includes(value);
}

export type PersistenceFlushCause =
  | "Startup"
  | "Interval"
  | "RouteChange"
  | "RatchetRotation"
  | "Shutdown";

export const PERSISTENCE_FLUSH_CAUSE_VALUES: readonly PersistenceFlushCause[] = Object.freeze([
  "Startup",
  "Interval",
  "RouteChange",
  "RatchetRotation",
  "Shutdown",
]);

export function isPersistenceFlushCause(value: unknown): value is PersistenceFlushCause {
  return typeof value === "string" && (PERSISTENCE_FLUSH_CAUSE_VALUES as readonly string[]).includes(value);
}

export type PersistenceFlushTarget =
  | "RoutingState"
  | "Ratchets";

export const PERSISTENCE_FLUSH_TARGET_VALUES: readonly PersistenceFlushTarget[] = Object.freeze([
  "RoutingState",
  "Ratchets",
]);

export function isPersistenceFlushTarget(value: unknown): value is PersistenceFlushTarget {
  return typeof value === "string" && (PERSISTENCE_FLUSH_TARGET_VALUES as readonly string[]).includes(value);
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
    applicationEvents: 1024,
    retainedEventBytes: 8388608,
    diagnostics: 1024,
  };
}

export type BackendKind =
  | "Native"
  | "Browser"
  | "Cooperative";

export const BACKEND_KIND_VALUES: readonly BackendKind[] = Object.freeze([
  "Native",
  "Browser",
  "Cooperative",
]);

export function isBackendKind(value: unknown): value is BackendKind {
  return typeof value === "string" && (BACKEND_KIND_VALUES as readonly string[]).includes(value);
}

export type InterfaceKind =
  | "AutoLan"
  | "TcpClient"
  | "TcpServer"
  | "Udp"
  | "Serial"
  | "Kiss"
  | "Ax25Kiss"
  | "RNode"
  | "MultiRNode"
  | "Pipe"
  | "BackboneClient"
  | "BackboneServer"
  | "I2p"
  | "Weave"
  | "AutomaticUsb"
  | "AutomaticBluetoothLe"
  | "WebSocketClient"
  | "WebSocketServer"
  | "BrowserRendezvous";

export const INTERFACE_KIND_VALUES: readonly InterfaceKind[] = Object.freeze([
  "AutoLan",
  "TcpClient",
  "TcpServer",
  "Udp",
  "Serial",
  "Kiss",
  "Ax25Kiss",
  "RNode",
  "MultiRNode",
  "Pipe",
  "BackboneClient",
  "BackboneServer",
  "I2p",
  "Weave",
  "AutomaticUsb",
  "AutomaticBluetoothLe",
  "WebSocketClient",
  "WebSocketServer",
  "BrowserRendezvous",
]);

export function isInterfaceKind(value: unknown): value is InterfaceKind {
  return typeof value === "string" && (INTERFACE_KIND_VALUES as readonly string[]).includes(value);
}

export type InterfaceMode =
  | "Full"
  | "PointToPoint"
  | "AccessPoint"
  | "Roaming"
  | "Boundary"
  | "Gateway"
  | "Internal";

export const INTERFACE_MODE_VALUES: readonly InterfaceMode[] = Object.freeze([
  "Full",
  "PointToPoint",
  "AccessPoint",
  "Roaming",
  "Boundary",
  "Gateway",
  "Internal",
]);

export function isInterfaceMode(value: unknown): value is InterfaceMode {
  return typeof value === "string" && (INTERFACE_MODE_VALUES as readonly string[]).includes(value);
}

export type WebSocketFramingSelection =
  | "RawPacket"
  | "Hdlc"
  | "Kiss"
  | "Auto";

export const WEB_SOCKET_FRAMING_SELECTION_VALUES: readonly WebSocketFramingSelection[] = Object.freeze([
  "RawPacket",
  "Hdlc",
  "Kiss",
  "Auto",
]);

export function isWebSocketFramingSelection(value: unknown): value is WebSocketFramingSelection {
  return typeof value === "string" && (WEB_SOCKET_FRAMING_SELECTION_VALUES as readonly string[]).includes(value);
}

export type InterfaceHealth =
  | "Initializing"
  | "Connected"
  | "Degraded"
  | "Reconnecting"
  | "Failed"
  | "Disconnected"
  | "Disabled"
  | "Unknown";

export const INTERFACE_HEALTH_VALUES: readonly InterfaceHealth[] = Object.freeze([
  "Initializing",
  "Connected",
  "Degraded",
  "Reconnecting",
  "Failed",
  "Disconnected",
  "Disabled",
  "Unknown",
]);

export function isInterfaceHealth(value: unknown): value is InterfaceHealth {
  return typeof value === "string" && (INTERFACE_HEALTH_VALUES as readonly string[]).includes(value);
}

export type DiscoveryScope =
  | "Link"
  | "Admin"
  | "Site"
  | "Organization"
  | "Global";

export const DISCOVERY_SCOPE_VALUES: readonly DiscoveryScope[] = Object.freeze([
  "Link",
  "Admin",
  "Site",
  "Organization",
  "Global",
]);

export function isDiscoveryScope(value: unknown): value is DiscoveryScope {
  return typeof value === "string" && (DISCOVERY_SCOPE_VALUES as readonly string[]).includes(value);
}

export type MulticastAddressType =
  | "Temporary"
  | "Permanent";

export const MULTICAST_ADDRESS_TYPE_VALUES: readonly MulticastAddressType[] = Object.freeze([
  "Temporary",
  "Permanent",
]);

export function isMulticastAddressType(value: unknown): value is MulticastAddressType {
  return typeof value === "string" && (MULTICAST_ADDRESS_TYPE_VALUES as readonly string[]).includes(value);
}

export type SerialDataBits =
  | "Five"
  | "Six"
  | "Seven"
  | "Eight";

export const SERIAL_DATA_BITS_VALUES: readonly SerialDataBits[] = Object.freeze([
  "Five",
  "Six",
  "Seven",
  "Eight",
]);

export function isSerialDataBits(value: unknown): value is SerialDataBits {
  return typeof value === "string" && (SERIAL_DATA_BITS_VALUES as readonly string[]).includes(value);
}

export type SerialParity =
  | "None"
  | "Even"
  | "Odd";

export const SERIAL_PARITY_VALUES: readonly SerialParity[] = Object.freeze([
  "None",
  "Even",
  "Odd",
]);

export function isSerialParity(value: unknown): value is SerialParity {
  return typeof value === "string" && (SERIAL_PARITY_VALUES as readonly string[]).includes(value);
}

export type SerialStopBits =
  | "One"
  | "Two";

export const SERIAL_STOP_BITS_VALUES: readonly SerialStopBits[] = Object.freeze([
  "One",
  "Two",
]);

export function isSerialStopBits(value: unknown): value is SerialStopBits {
  return typeof value === "string" && (SERIAL_STOP_BITS_VALUES as readonly string[]).includes(value);
}

export type DestinationName = {
  readonly appName: string;
  readonly aspects: readonly string[];
};

export type RequestHandlerConfig = {
  readonly path: string;
  readonly policy: RequestPolicy;
};

export type SerialLineConfig = {
  readonly baud: number;
  readonly dataBits: SerialDataBits;
  readonly parity: SerialParity;
  readonly stopBits: SerialStopBits;
};

export type RNodeRadioConfig = {
  readonly frequencyHz: number;
  readonly bandwidthHz: number;
  readonly txPowerDbm: number;
  readonly spreadingFactor: number;
  readonly codingRate: number;
};

export type MultiRNodeMemberConfig = {
  readonly name: string;
  readonly virtualPort: number;
  readonly radio: RNodeRadioConfig;
  readonly flowControl: boolean;
  readonly outgoing: boolean;
};

export type InterfaceRoutingPolicy = {
  readonly mode?: InterfaceMode;
  readonly gravity?: number;
  readonly recursivePathRequests?: boolean;
  readonly announcesFromInternal?: boolean;
  readonly announcesToInternal?: boolean;
};

export type BackendInfo = {
  readonly backend: BackendKind;
  readonly capabilities: readonly CapabilityName[];
  readonly interfaceKinds: readonly InterfaceKind[];
};

export type InterfaceSnapshot = {
  readonly interfaceId: InterfaceId;
  readonly name?: string;
  readonly kind?: InterfaceKind;
  readonly health: InterfaceHealth;
  readonly failureDetail?: string;
  readonly rxBytes: bigint;
  readonly txBytes: bigint;
  readonly rxBps?: number;
  readonly txBps?: number;
  readonly routeCount: number;
  readonly linkCount: number;
  readonly transportedLinkCount: number;
};

export type RouteSnapshot = {
  readonly destination: DestinationHash;
  readonly hops: number;
  readonly viaIdentity?: IdentityHash;
  readonly interfaceId: InterfaceId;
  readonly learnedAtMillis: number;
  readonly lastRouteActivityAtMillis: number;
  readonly expiresAtMillis: number;
};

export type DestinationIdentitySnapshot = {
  readonly destination: DestinationHash;
  readonly identity: IdentityHash;
};

export type RuntimeHealthSnapshot = {
  readonly running: boolean;
  readonly uptimeMillis: number;
  readonly interfaceCount: number;
  readonly onlineInterfaceCount: number;
  readonly routeCount: number;
  readonly linkCount: number;
  readonly transportedLinkCount: number;
  readonly rxBytes: bigint;
  readonly txBytes: bigint;
  readonly rxBps: number;
  readonly txBps: number;
};

export type PersistenceSnapshot = {
  readonly persistent: boolean;
  readonly restored: boolean;
  readonly lastFlushCause?: PersistenceFlushCause;
  readonly lastFailureDetail?: string;
};

export type HostSnapshot = {
  readonly revision: bigint;
  readonly backend: BackendInfo;
  readonly interfaces: readonly InterfaceSnapshot[];
  readonly routes: readonly RouteSnapshot[];
  readonly activeLinkCount: number;
  readonly destinationIdentities: readonly DestinationIdentitySnapshot[];
  readonly runtime: RuntimeHealthSnapshot;
  readonly persistence: PersistenceSnapshot;
};

export type ResourceStream = {
  readonly totalBytes: bigint;
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

export type PersistenceConfig =
  | Tag<"Ephemeral">
  | Tag<
      "Directory",
      {
        readonly path: string;
      }
    >;

export type InterfaceConfig =
  | Tag<
      "AutoLan",
      {
        readonly groupId?: string;
        readonly discoveryScope?: DiscoveryScope;
        readonly discoveryPort?: number;
        readonly dataPort?: number;
        readonly devices: readonly string[];
        readonly ignoredDevices: readonly string[];
        readonly multicastAddressType?: MulticastAddressType;
      }
    >
  | Tag<
      "TcpClient",
      {
        readonly target: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "TcpServer",
      {
        readonly bind: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "Udp",
      {
        readonly local: string;
        readonly peer: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "Serial",
      {
        readonly port: string;
        readonly line: SerialLineConfig;
      }
    >
  | Tag<
      "Kiss",
      {
        readonly port: string;
        readonly line: SerialLineConfig;
        readonly flowControl: boolean;
        readonly preambleMillis: number;
        readonly transmitTailMillis: number;
        readonly persistence: number;
        readonly slotTimeMillis: number;
        readonly stationCallsign?: string;
        readonly stationIntervalSeconds?: number;
      }
    >
  | Tag<
      "Ax25Kiss",
      {
        readonly port: string;
        readonly line: SerialLineConfig;
        readonly flowControl: boolean;
        readonly preambleMillis: number;
        readonly transmitTailMillis: number;
        readonly persistence: number;
        readonly slotTimeMillis: number;
        readonly callsign: string;
        readonly ssid: number;
      }
    >
  | Tag<
      "RNode",
      {
        readonly port: string;
        readonly radio: RNodeRadioConfig;
        readonly flowControl: boolean;
        readonly stationCallsign?: string;
        readonly stationIntervalSeconds?: number;
        readonly airtimeLimitShortCentiPercent?: number;
        readonly airtimeLimitLongCentiPercent?: number;
      }
    >
  | Tag<
      "MultiRNode",
      {
        readonly port: string;
        readonly stationCallsign?: string;
        readonly stationIntervalSeconds?: number;
        readonly members: readonly MultiRNodeMemberConfig[];
      }
    >
  | Tag<
      "Pipe",
      {
        readonly command: readonly string[];
        readonly respawnDelayMillis: number;
      }
    >
  | Tag<
      "BackboneClient",
      {
        readonly target: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "BackboneServer",
      {
        readonly bind: string;
        readonly bitrate: Bitrate;
      }
    >
  | Tag<
      "I2p",
      {
        readonly peers: readonly string[];
        readonly connectable: boolean;
      }
    >
  | Tag<
      "Weave",
      {
        readonly port: string;
      }
    >
  | Tag<"AutomaticUsb">
  | Tag<"AutomaticBluetoothLe">
  | Tag<
      "WebSocketClient",
      {
        readonly target: string;
        readonly framing: WebSocketFramingSelection;
      }
    >
  | Tag<
      "WebSocketServer",
      {
        readonly bind: string;
        readonly framing: WebSocketFramingSelection;
      }
    >
  | Tag<
      "BrowserRendezvous",
      {
        readonly url: string;
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
        readonly maximumRequestBytes?: number;
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
        readonly maximumResponseBytes?: number;
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
    >
  | Tag<
      "AttachInterface",
      {
        readonly config: InterfaceConfig;
        readonly routing?: InterfaceRoutingPolicy;
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
  | Tag<"InvalidChannelMessageType">
  | Tag<
      "InvalidConfiguration",
      {
        readonly detail: string;
      }
    >
  | Tag<"ResourceUploadCancelled">
  | Tag<"ResourceEarlyEof">
  | Tag<"ResourceLengthOverrun">
  | Tag<
      "PermissionDenied",
      {
        readonly detail: string;
      }
    >
  | Tag<
      "DeviceUnavailable",
      {
        readonly detail: string;
      }
    >
  | Tag<
      "ConnectFailed",
      {
        readonly detail: string;
      }
    >
  | Tag<
      "BackendFailed",
      {
        readonly detail: string;
      }
    >
  | Tag<"ResponseTooLarge">;

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
        readonly uncompressedDataBytes: bigint;
      }
    >
  | Tag<
      "ChannelMessage",
      {
        readonly linkId: LinkId;
        readonly messageType: number;
        readonly data: Uint8Array;
      }
    >
  | Tag<
      "LinkDelivery",
      {
        readonly linkId: LinkId;
        readonly sourceInterface: InterfaceId;
        readonly plaintext: Uint8Array;
      }
    >;

export type DiagnosticEvent =
  | Tag<
      "AnnounceHeard",
      {
        readonly destination: DestinationHash;
        readonly hops: number;
        readonly sourceInterface: InterfaceId;
        readonly appData: Uint8Array;
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
        readonly totalSizeBytes: bigint;
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
        readonly transferredBytes: bigint;
        readonly totalBytes: bigint;
        readonly physicalTransferredBytes: bigint;
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
    >
  | Tag<
      "PersistenceRestored",
      {
        readonly routes: number;
        readonly destinationIdentities: number;
        readonly tunnels: number;
        readonly ratchets: number;
        readonly refused: number;
        readonly dropped: number;
      }
    >
  | Tag<
      "PersistenceFlushed",
      {
        readonly cause: PersistenceFlushCause;
        readonly target: PersistenceFlushTarget;
      }
    >
  | Tag<
      "PersistenceFlushFailed",
      {
        readonly cause: PersistenceFlushCause;
        readonly target: PersistenceFlushTarget;
      }
    >;

const HOST_OPERATION_NAMES = [
  "contractInfo",
  "backendInfo",
  "hostCreate",
  "hostRelease",
  "hostLifecycle",
  "hostSnapshot",
  "hostSnapshotRead",
  "hostSnapshotRelease",
  "hostIdentityHash",
  "hostDestinationCount",
  "hostDestinationHash",
  "hostAttachSuppliedPipe",
  "suppliedPipeClaimAttachment",
  "suppliedPipeNextOpenRequest",
  "suppliedPipeRegisterReadiness",
  "suppliedPipeInterruptWait",
  "suppliedPipeRelease",
  "suppliedPipeOpenRequestProvide",
  "suppliedPipeOpenRequestDecline",
  "suppliedPipeOpenRequestRelease",
  "hostBeginResourceUpload",
  "resourceUploadWrite",
  "resourceUploadIsWritable",
  "resourceUploadFinish",
  "resourceUploadAbort",
  "resourceUploadRelease",
  "hostStop",
  "commandWait",
  "commandRegisterReadiness",
  "commandInterruptWait",
  "commandRelease",
  "hostClaimApplicationEvents",
  "hostClaimDiagnostics",
  "eventStreamRegisterReadiness",
  "readinessRegistrationRelease",
  "eventStreamInterruptWait",
  "eventStreamRelease",
  "eventStreamNext",
  "eventRelease",
  "eventKind",
  "eventBytes",
  "eventString",
  "eventU64",
  "eventU128",
  "eventResourceStream",
  "resourceStreamRelease",
  "resourceStreamNext",
  "hostAnnounce",
  "hostSendSinglePacket",
  "hostCloseLink",
  "hostAttachTcpServer",
  "hostAttachTcpClient",
  "hostAttachUdp",
  "hostDetachInterface",
  "hostEstablishLink",
  "hostRequestPath",
  "hostIdentify",
  "hostSendLinkPacket",
  "hostRequest",
  "hostRespond",
  "hostSendResource",
  "hostSetLinkResourceStrategy",
  "hostSetDestinationResourceStrategy",
  "hostSendChannelMessage",
  "hostAllowRequester",
  "hostAttachInterface",
] as const;

type HostOperationName = (typeof HOST_OPERATION_NAMES)[number];

type RawUnit = undefined;
type RawOwned<Value> = { readonly value: Value; readonly ownership: "owned" };
type RawBorrowed<Value> = { readonly value: Value; readonly ownership: "borrowed" };
type RawCallResult<Value> =
  | Tag<"Succeeded", Value>
  | Tag<"Failed", RawStatus>;
type RawCommandResult = { readonly rawType: "CommandResult" };
type RawContractInfo = { readonly rawType: "ContractInfo" };
type RawEvent = { readonly rawType: "Event" };
type RawEventField = { readonly rawType: "EventField" };
type RawEventStream = { readonly rawType: "EventStream" };
type RawHost = { readonly rawType: "Host" };
type RawHostInspection = { readonly rawType: "HostInspection" };
type RawHostOptions = { readonly rawType: "HostOptions" };
type RawIssuedCommand = { readonly rawType: "IssuedCommand" };
type RawLifecycle = { readonly rawType: "Lifecycle" };
type RawReadinessCallback = { readonly rawType: "ReadinessCallback" };
type RawReadinessRegistration = { readonly rawType: "ReadinessRegistration" };
type RawResourceChunk = { readonly rawType: "ResourceChunk" };
type RawResourceStream = { readonly rawType: "ResourceStream" };
type RawResourceUpload = { readonly rawType: "ResourceUpload" };
type RawStatus = { readonly rawType: "Status" };
type RawSuppliedPipe = { readonly rawType: "SuppliedPipe" };
type RawSuppliedPipeOpenRequest = { readonly rawType: "SuppliedPipeOpenRequest" };
type RawOpaquePointer = { readonly rawType: "opaquePointer" };

interface RawHostProtocol {
  readonly contractInfo: () => RawCallResult<RawContractInfo>;
  readonly backendInfo: () => RawCallResult<BackendInfo>;
  readonly hostCreate: (options: RawHostOptions) => RawCallResult<RawOwned<RawHost>>;
  readonly hostRelease: (host: RawHost) => RawUnit;
  readonly hostLifecycle: (host: RawHost) => RawCallResult<RawLifecycle>;
  readonly hostSnapshot: (host: RawHost, timeoutMillis: number) => RawCallResult<RawOwned<RawHostInspection>>;
  readonly hostSnapshotRead: (host_inspection: RawHostInspection) => RawCallResult<RawBorrowed<HostSnapshot>>;
  readonly hostSnapshotRelease: (host_inspection: RawHostInspection) => RawUnit;
  readonly hostIdentityHash: (host: RawHost) => RawCallResult<RawBorrowed<Uint8Array>>;
  readonly hostDestinationCount: (host: RawHost) => number;
  readonly hostDestinationHash: (host: RawHost, index: number) => RawCallResult<RawBorrowed<Uint8Array>>;
  readonly hostAttachSuppliedPipe: (host: RawHost, name: string, respawnDelayMillis: number, bitrate: Bitrate) => RawCallResult<RawOwned<RawSuppliedPipe>>;
  readonly suppliedPipeClaimAttachment: (supplied_pipe: RawSuppliedPipe) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly suppliedPipeNextOpenRequest: (supplied_pipe: RawSuppliedPipe, timeoutMillis: number) => RawCallResult<RawOwned<RawSuppliedPipeOpenRequest>>;
  readonly suppliedPipeRegisterReadiness: (supplied_pipe: RawSuppliedPipe, callback: RawReadinessCallback, context: RawOpaquePointer) => RawCallResult<RawOwned<RawReadinessRegistration>>;
  readonly suppliedPipeInterruptWait: (supplied_pipe: RawSuppliedPipe) => RawUnit;
  readonly suppliedPipeRelease: (supplied_pipe: RawSuppliedPipe) => RawUnit;
  readonly suppliedPipeOpenRequestProvide: (supplied_pipe_open_request: RawSuppliedPipeOpenRequest, descriptor: bigint) => RawCallResult<boolean>;
  readonly suppliedPipeOpenRequestDecline: (supplied_pipe_open_request: RawSuppliedPipeOpenRequest) => RawCallResult<boolean>;
  readonly suppliedPipeOpenRequestRelease: (supplied_pipe_open_request: RawSuppliedPipeOpenRequest) => RawUnit;
  readonly hostBeginResourceUpload: (host: RawHost, linkId: LinkId, declaredLength: bigint, packedMetadata: Uint8Array | undefined, compression: ResourceCompression) => RawCallResult<RawOwned<RawResourceUpload>>;
  readonly resourceUploadWrite: (resource_upload: RawResourceUpload, chunk: Uint8Array) => RawCallResult<RawUnit>;
  readonly resourceUploadIsWritable: (resource_upload: RawResourceUpload) => RawCallResult<boolean>;
  readonly resourceUploadFinish: (resource_upload: RawResourceUpload) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly resourceUploadAbort: (resource_upload: RawResourceUpload) => RawUnit;
  readonly resourceUploadRelease: (resource_upload: RawResourceUpload) => RawUnit;
  readonly hostStop: (host: RawHost) => RawCallResult<RawUnit>;
  readonly commandWait: (issued_command: RawIssuedCommand, timeoutMillis: number) => RawCallResult<RawBorrowed<RawCommandResult>>;
  readonly commandRegisterReadiness: (issued_command: RawIssuedCommand, callback: RawReadinessCallback, context: RawOpaquePointer) => RawCallResult<RawOwned<RawReadinessRegistration>>;
  readonly commandInterruptWait: (issued_command: RawIssuedCommand) => RawUnit;
  readonly commandRelease: (issued_command: RawIssuedCommand) => RawUnit;
  readonly hostClaimApplicationEvents: (host: RawHost) => RawCallResult<RawOwned<RawEventStream>>;
  readonly hostClaimDiagnostics: (host: RawHost) => RawCallResult<RawOwned<RawEventStream>>;
  readonly eventStreamRegisterReadiness: (event_stream: RawEventStream, callback: RawReadinessCallback, context: RawOpaquePointer) => RawCallResult<RawOwned<RawReadinessRegistration>>;
  readonly readinessRegistrationRelease: (readiness_registration: RawReadinessRegistration) => RawUnit;
  readonly eventStreamInterruptWait: (event_stream: RawEventStream) => RawUnit;
  readonly eventStreamRelease: (event_stream: RawEventStream) => RawUnit;
  readonly eventStreamNext: (event_stream: RawEventStream, timeoutMillis: number) => RawCallResult<RawOwned<RawEvent>>;
  readonly eventRelease: (eventValue: RawEvent) => RawUnit;
  readonly eventKind: (eventValue: RawEvent) => number;
  readonly eventBytes: (eventValue: RawEvent, field: RawEventField) => RawCallResult<RawBorrowed<Uint8Array>>;
  readonly eventString: (eventValue: RawEvent, field: RawEventField) => RawCallResult<RawBorrowed<string>>;
  readonly eventU64: (eventValue: RawEvent, field: RawEventField) => RawCallResult<bigint>;
  readonly eventU128: (eventValue: RawEvent, field: RawEventField) => RawCallResult<bigint>;
  readonly eventResourceStream: (eventValue: RawEvent) => RawCallResult<RawOwned<RawResourceStream>>;
  readonly resourceStreamRelease: (resource_stream: RawResourceStream) => RawUnit;
  readonly resourceStreamNext: (resource_stream: RawResourceStream, maximumBytes: number) => RawCallResult<RawBorrowed<RawResourceChunk>>;
  readonly hostAnnounce: (host: RawHost, destination: DestinationHash, interfaceId: InterfaceId | undefined) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostSendSinglePacket: (host: RawHost, destination: DestinationHash, payload: Uint8Array) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostCloseLink: (host: RawHost, linkId: LinkId) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostAttachTcpServer: (host: RawHost, bind: string, bitrate: Bitrate) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostAttachTcpClient: (host: RawHost, target: string, bitrate: Bitrate) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostAttachUdp: (host: RawHost, local: string, peer: string, bitrate: Bitrate) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostDetachInterface: (host: RawHost, interfaceId: InterfaceId) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostEstablishLink: (host: RawHost, destination: DestinationHash) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostRequestPath: (host: RawHost, destination: DestinationHash) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostIdentify: (host: RawHost, linkId: LinkId, identity: IdentityHash) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostSendLinkPacket: (host: RawHost, linkId: LinkId, payload: Uint8Array) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostRequest: (host: RawHost, linkId: LinkId, pathHash: RequestPathHash, payload: Uint8Array, timeout: ResponseTimeout, maximumResponseBytes: number | undefined) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostRespond: (host: RawHost, linkId: LinkId, requestId: RequestId, requestRttMillis: number, payload: Uint8Array) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostSendResource: (host: RawHost, linkId: LinkId, payload: Uint8Array, packedMetadata: Uint8Array | undefined, compression: ResourceCompression) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostSetLinkResourceStrategy: (host: RawHost, linkId: LinkId, strategy: ResourceStrategy) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostSetDestinationResourceStrategy: (host: RawHost, destination: DestinationHash, strategy: ResourceStrategy) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostSendChannelMessage: (host: RawHost, linkId: LinkId, messageType: number, payload: Uint8Array) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostAllowRequester: (host: RawHost, destination: DestinationHash, pathHash: RequestPathHash, identity: IdentityHash) => RawCallResult<RawOwned<RawIssuedCommand>>;
  readonly hostAttachInterface: (host: RawHost, config: InterfaceConfig, routing: InterfaceRoutingPolicy | undefined) => RawCallResult<RawOwned<RawIssuedCommand>>;
}

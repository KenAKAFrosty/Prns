import { Tag, from, match, match_into } from "../casework.js";
import { BoundedAsyncLane } from "../async_lanes.js";
import type { StreamClaim } from "../async_lanes.js";
import {
  DESTINATION_HASH_LENGTH,
  HOST_CONTRACT_ABI,
  HOST_SCHEMA_VERSION,
  INTERFACE_ID_LENGTH,
  PRODUCT_VERSION,
  RESOURCE_HASH_LENGTH,
  SAFE_INT_MAX,
  SAFE_INT_MIN,
  balancedLimits,
  destinationHash,
  identityHash,
  interfaceId,
  linkId,
  packetHash,
  requestId,
  requestPathHash,
  resourceHash,
} from "../contract.js";
import type {
  ApplicationEvent as HostApplicationEvent,
  BackendCapabilities,
  BackendInfo,
  CapabilityName,
  CommandFailure,
  CommandOutcome,
  CommandSettlement,
  CommandSettlementFor,
  DeliveryEvidenceKind,
  DestinationHash,
  DestinationIdentitySnapshot as StableDestinationIdentitySnapshot,
  DiagnosticEvent as HostDiagnosticEvent,
  HostCommand,
  HostSnapshot as StableHostSnapshot,
  IdentityHash,
  InterfaceConfig,
  InterfaceHealth,
  InterfaceId,
  InterfaceKind,
  InterfaceRoutingPolicy,
  LifecycleState as HostLifecycleState,
  LinkId,
  PrnsLimits as HostLimits,
  RequestId,
  RequestHandlerConfig,
  RequestPathHash,
  ResourceCompression,
  ResourceHash,
  ResourceStrategy,
  ResourceStream,
  ResponseTimeout,
  RouteSnapshot as StableRouteSnapshot,
} from "../contract.js";
import { MemoryResourceStream } from "../memory_resource.js";
import { AutoWifiInterface } from "./auto_wifi.js";
import {
  blobResourceSource,
  byteResourceSource,
  sendResourceFromSource,
} from "./resource_send.js";
import { browserResourceCompressor } from "./resource_compressor.js";
import type {
  ResourceSendSettlement,
  ResourceSource,
  RuntimeResourcePlanInput,
  RuntimeResourceSegmentInput,
  RuntimeResourceSegmentIssueInput,
} from "./resource_send.js";

export { Tag, from, match, match_into };
export {
  DESTINATION_HASH_LENGTH,
  HOST_CONTRACT_ABI,
  HOST_SCHEMA_VERSION,
  INTERFACE_ID_LENGTH,
  PRODUCT_VERSION,
  RESOURCE_HASH_LENGTH,
  SAFE_INT_MAX,
  SAFE_INT_MIN,
  balancedLimits,
  destinationHash,
  identityHash,
  interfaceId,
  linkId,
  packetHash,
  requestId,
  requestPathHash,
  resourceHash,
};
export type { DataFrom, TagFrom } from "../casework.js";
export type { StreamClaim } from "../async_lanes.js";
export type {
  BackendCapabilities,
  BackendInfo,
  Bitrate,
  CapabilityName,
  CommandFailure,
  CommandOutcome,
  CommandSettlement,
  CommandSettlementFor,
  DestinationHash,
  DestinationIdentitySnapshot,
  DeliveryEvidenceKind,
  HostCommand,
  HostSnapshot,
  IdentityHash,
  InterfaceConfig,
  InterfaceHealth,
  InterfaceId,
  InterfaceKind,
  LinkId,
  RequestId,
  RequestHandlerConfig,
  RequestPolicy,
  RequestPathHash,
  ResourceCompression,
  ResourceHash,
  ResourceStrategy,
  ResourceStream,
  ResponseTimeout,
  RouteSnapshot,
} from "../contract.js";
export {
  AutoWifiController,
  AutoWifiInterface,
  parseBrowserGatewayCatalog,
  validateBrowserGatewayUrl,
} from "./auto_wifi.js";
export type {
  AutoWifiControllerCloseOutcome,
  AutoWifiControllerStatus,
  AutoWifiFailure,
  AutoWifiGatewayStatus,
  BrowserGatewayCatalogOutcome,
  BrowserRendezvousId,
} from "./auto_wifi.js";

declare const brand: unique symbol;

type Brand<Name extends string> = { readonly [brand]: Name };
type BrandedBytes<Name extends string> = Uint8Array & Brand<Name>;
type BrandedNumber<Name extends string> = number & Brand<Name>;
type BrandedBigInt<Name extends string> = bigint & Brand<Name>;

export const MIN_ENTROPY_BYTES = 128;
export const BLE_IDENTITY_LENGTH = 16;

export type IdentitySecretKey = BrandedBytes<"IdentitySecretKey">;
export type BleIdentity = BrandedBytes<"BleIdentity">;
export type ChannelTag = BrandedBytes<"ChannelTag">;
export type PacketFrame = BrandedBytes<"PacketFrame">;
export type EntropyBytes = BrandedBytes<"EntropyBytes">;
export type AppData = BrandedBytes<"AppData">;

export type AppName = string & Brand<"AppName">;
export type Aspect = string & Brand<"Aspect">;
export type InstantMillis = BrandedNumber<"InstantMillis">;
export type BitrateBps = BrandedNumber<"BitrateBps">;
export type HardwareMtu = BrandedNumber<"HardwareMtu">;
export type HopCount = BrandedNumber<"HopCount">;
export type CommandId = BrandedBigInt<"CommandId">;

export type PrnsValidationCode =
  | "empty-bytes"
  | "empty-string"
  | "invalid-component"
  | "invalid-length"
  | "invalid-number"
  | "missing-host-api"
  | "unknown-interface-kind"
  | "unknown-outbound-target";

export class PrnsValidationError extends Error {
  readonly code: PrnsValidationCode;

  constructor(code: PrnsValidationCode, message: string) {
    super(message);
    this.name = "PrnsValidationError";
    this.code = code;
  }
}

export type RuntimeOperation =
  | "initialize"
  | "inspect-readiness"
  | "register-interface"
  | "remove-interface"
  | "register-destination"
  | "register-node-page"
  | "announce"
  | "send-single-packet"
  | "establish-link"
  | "request-path"
  | "identify"
  | "send-link-packet"
  | "request"
  | "respond"
  | "send-resource"
  | "set-link-resource-strategy"
  | "set-destination-resource-strategy"
  | "send-channel-message"
  | "allow-requester"
  | "close-link"
  | "ingest"
  | "drain-events"
  | "drain-outbound"
  | "snapshot";

export type RuntimeRejected = Tag<
  "RuntimeRejected",
  { readonly operation: RuntimeOperation; readonly detail: string }
>;

export type HostApi =
  | "Crypto"
  | "LocalStorage"
  | "Base64Encoder"
  | "Base64Decoder"
  | "WebUSB"
  | "WebBluetooth"
  | "WebSocket"
  | "Fetch";

export type HostApiUnavailable<Api extends HostApi = HostApi> = Tag<
  "HostApiUnavailable",
  { readonly api: Api }
>;

type IdentityStoreOperationFailure<Operation extends "Load" | "Save"> = Tag<
  "IdentityStoreFailed",
  { readonly operation: Operation; readonly detail: string }
>;

export type IdentityLoadFailure =
  | HostApiUnavailable<"LocalStorage" | "Base64Decoder">
  | IdentityStoreOperationFailure<"Load">
  | Tag<"StoredIdentityInvalid", { readonly detail: string }>;

export type IdentitySaveFailure =
  | HostApiUnavailable<"LocalStorage" | "Base64Encoder">
  | IdentityStoreOperationFailure<"Save">;

export type IdentityStoreFailure =
  | IdentityLoadFailure
  | IdentitySaveFailure;

export type StableIdentityStoreFailure =
  | HostApiUnavailable<"LocalStorage" | "Base64Encoder" | "Base64Decoder">
  | Tag<
      "StableIdentityStoreFailed",
      { readonly operation: "Load" | "Save"; readonly detail: string }
    >
  | Tag<"StoredStableIdentityInvalid", { readonly detail: string }>;

export type PersistenceStoreFailure =
  | HostApiUnavailable<"LocalStorage" | "Base64Encoder" | "Base64Decoder">
  | Tag<
      "PersistenceStoreFailed",
      { readonly operation: "Load" | "Save"; readonly detail: string }
    >
  | Tag<"StoredPersistenceInvalid", { readonly detail: string }>;

export type StableIdentityUnavailable<
  Name extends InterfaceName = InterfaceName,
> = Tag<
  "StableIdentityUnavailable",
  { readonly interface: Name; readonly detail: string }
>;

export type IdentityLoadOutcome =
  | Tag<"Loaded", IdentitySecretKey>
  | Tag<"Missing">
  | IdentityLoadFailure;

export type IdentitySaveOutcome = Tag<"Saved"> | IdentitySaveFailure;

export type EntropyFailure =
  | HostApiUnavailable<"Crypto">
  | Tag<"EntropySourceFailed", { readonly detail: string }>
  | Tag<
      "InsufficientEntropy",
      { readonly minimum: number; readonly actual: number }
    >;

export type EntropyOutcome = Tag<"Filled", EntropyBytes> | EntropyFailure;

export type PrnsCreateOutcome =
  | Tag<"Ready", Prns>
  | Tag<"WasmLoadFailed", { readonly detail: string }>
  | Tag<
      "ContractMismatch",
      {
        readonly requiredAbi: number;
        readonly actualAbi: number;
        readonly requiredProductVersion: string;
        readonly actualProductVersion: string;
      }
    >
  | IdentityStoreFailure
  | PersistenceStoreFailure
  | EntropyFailure
  | RuntimeRejected;

export type InterfaceConnectStage =
  | "DeviceSelection"
  | "TransportOpen"
  | "ServiceDiscovery"
  | "Handshake"
  | "RuntimeRegistration";

export type PermissionDenied<Name extends InterfaceName = InterfaceName> = Tag<
  "PermissionDenied",
  {
    readonly interface: Name;
    readonly stage: InterfaceConnectStage;
    readonly detail: string;
  }
>;

export type Cancelled<Name extends InterfaceName = InterfaceName> = Tag<
  "Cancelled",
  {
    readonly interface: Name;
    readonly stage: InterfaceConnectStage;
  }
>;

export type AlreadyActive<Name extends InterfaceName = InterfaceName> = Tag<
  "AlreadyActive",
  { readonly interface: Name; readonly target: string }
>;

export type InvalidTarget<Name extends InterfaceName = InterfaceName> = Tag<
  "InvalidTarget",
  {
    readonly interface: Name;
    readonly target: string;
    readonly detail: string;
  }
>;

export type UnsupportedDevice<Name extends InterfaceName = InterfaceName> = Tag<
  "UnsupportedDevice",
  { readonly interface: Name; readonly capability: string }
>;

export type ConnectTimedOut<Name extends InterfaceName = InterfaceName> = Tag<
  "TimedOut",
  {
    readonly interface: Name;
    readonly stage: InterfaceConnectStage;
    readonly timeoutMs: number;
  }
>;

export type ConnectionFailed<Name extends InterfaceName = InterfaceName> = Tag<
  "ConnectionFailed",
  {
    readonly interface: Name;
    readonly stage: InterfaceConnectStage;
    readonly detail: string;
  }
>;

export type UnsupportedInterface<Name extends InterfaceName = InterfaceName> = Tag<
  "UnsupportedInterface",
  { readonly interface: Name; readonly host: "Browser" }
>;

export type UsbAutoConnectOutcome =
  | Tag<"Connected", UsbAutoSession>
  | HostApiUnavailable<"WebUSB">
  | PermissionDenied<"usb-auto">
  | Cancelled<"usb-auto">
  | AlreadyActive<"usb-auto">
  | UnsupportedDevice<"usb-auto">
  | ConnectionFailed<"usb-auto">
  | RuntimeRejected;

export type WebSocketConnectOutcome =
  | Tag<"Connected", WebSocketSession>
  | HostApiUnavailable<"WebSocket">
  | PermissionDenied<"websocket">
  | Cancelled<"websocket">
  | AlreadyActive<"websocket">
  | InvalidTarget<"websocket">
  | ConnectTimedOut<"websocket">
  | ConnectionFailed<"websocket">
  | RuntimeRejected;

export type BluetoothConnectOutcome =
  | Tag<"Connected", BluetoothSession>
  | HostApiUnavailable<"WebBluetooth">
  | PermissionDenied<"bluetooth">
  | Cancelled<"bluetooth">
  | UnsupportedDevice<"bluetooth">
  | ConnectTimedOut<"bluetooth">
  | ConnectionFailed<"bluetooth">
  | AlreadyActive<"bluetooth">
  | StableIdentityUnavailable<"bluetooth">
  | RuntimeRejected;

export type BluetoothConnectFailure = Exclude<
  BluetoothConnectOutcome,
  Tag<"Connected", unknown>
>;

export type RNodeConnectOutcome =
  | UnsupportedInterface<"rnode">
  | RuntimeRejected;

export type InterfaceCleanupFailure =
  | Tag<"RuntimeDetachFailed", { readonly detail: string }>
  | Tag<"TransportCloseFailed", { readonly detail: string }>;

export type InterfaceCleanupFailures = readonly [
  InterfaceCleanupFailure,
  ...InterfaceCleanupFailure[],
];

export type InterfaceSessionFailure =
  | Tag<"Disconnected", { readonly detail: string }>
  | Tag<
      "TransferFailed",
      { readonly direction: "Inbound" | "Outbound"; readonly detail: string }
    >
  | Tag<
      "ProtocolViolation",
      {
        readonly protocol: "UsbAuto" | "Bluetooth" | "WebSocket";
        readonly detail: string;
      }
    >
  | Tag<"UnsupportedFrame", { readonly format: "Text" | "Unknown" }>
  | Tag<
      "FrameTooLarge",
      { readonly length: number; readonly maximum: number }
    >
  | Tag<"OutboundQueueFull", { readonly capacity: number }>
  | Tag<
      "CloseFailed",
      {
        readonly causes: InterfaceCleanupFailures;
      }
    >
  | Tag<"UnexpectedSessionFailure", { readonly detail: string }>
  | EntropyFailure
  | RuntimeRejected;

export type InterfaceSessionStatus =
  | Tag<"Negotiating">
  | Tag<"Active">
  | Tag<"Closed">
  | Tag<"Failed", InterfaceSessionFailure>;

export type InterfaceCloseOutcome =
  | Tag<"Closed">
  | Extract<InterfaceSessionFailure, Tag<"CloseFailed", unknown>>;

export type DestinationRegistrationOutcome =
  | Tag<"Registered", DestinationHash>
  | RuntimeRejected;

export type OperationFailed = Tag<
  "OperationFailed",
  { readonly operation: string; readonly detail: string; readonly code?: string }
>;
export type StopOutcome =
  | Tag<"Stopped">
  | Tag<"AlreadyStopped">
  | OperationFailed;

type CommandCase<Name extends HostCommand["tag"]> = Extract<
  HostCommand,
  { readonly tag: Name }
>;
export type AnnounceOutcome = CommandSettlementFor<CommandCase<"Announce">>;
export type SendSinglePacketOutcome = CommandSettlementFor<
  CommandCase<"SendSinglePacket">
>;
export type CloseLinkOutcome = CommandSettlementFor<CommandCase<"CloseLink">>;
export type AttachOutcome = CommandSettlementFor<CommandCase<"AttachInterface">>;
export type DetachInterfaceOutcome = CommandSettlementFor<
  CommandCase<"DetachInterface">
>;
export type EstablishLinkOutcome = CommandSettlementFor<
  CommandCase<"EstablishLink">
>;
export type RequestPathOutcome = CommandSettlementFor<
  CommandCase<"RequestPath">
>;
export type IdentifyOutcome = CommandSettlementFor<CommandCase<"Identify">>;
export type SendLinkPacketOutcome = CommandSettlementFor<
  CommandCase<"SendLinkPacket">
>;
export type RequestOutcome = CommandSettlementFor<CommandCase<"Request">>;
export type RespondOutcome = CommandSettlementFor<CommandCase<"Respond">>;
export type SendResourceOutcome = CommandSettlementFor<
  CommandCase<"SendResource">
>;
export type SendResourceOptions = {
  readonly packedMetadata?: Uint8Array;
  readonly compression?: ResourceCompression;
};
export type SetResourceStrategyOutcome = CommandSettlementFor<
  CommandCase<"SetLinkResourceStrategy" | "SetDestinationResourceStrategy">
>;
export type SendChannelMessageOutcome = CommandSettlementFor<
  CommandCase<"SendChannelMessage">
>;
export type AllowRequesterOutcome = CommandSettlementFor<
  CommandCase<"AllowRequester">
>;

export type SnapshotOutcome =
  | Tag<"Captured", PrnsSnapshot>
  | RuntimeRejected;
export type HostSnapshotOutcome =
  | Tag<"Captured", StableHostSnapshot>
  | RuntimeRejected;

export type PrnsWasmModule = {
  PrnsRuntime: {
    new(
      identitySecretKey: IdentitySecretKey,
      bleIdentity?: BleIdentity,
    ): PrnsRuntimeBinding;
  };
  UsbAutoDecoder: {
    new(): UsbAutoDecoderBinding;
  };
  BluetoothReassembler: {
    new(): BluetoothReassemblerBinding;
  };
  identitySecretKeyLength(): number;
  hostContractAbi(): number;
  hostSchemaVersion(): number;
  browserPersistenceVersion(): number;
  productVersion(): string;
  bluetoothServiceUuid(): string;
  bluetoothControlUuid(): string;
  bluetoothDataUuid(): string;
  bluetoothBitrateBps(): number;
  bluetoothHardwareMtu(): number;
  bluetoothDialerHello(identity: Uint8Array): Uint8Array;
  bluetoothDecodeControl(bytes: Uint8Array): unknown;
  bluetoothDataFragments(packet: PacketFrame): Uint8Array[];
  websocketBitrateBps(): number;
  websocketFrameCap(): number;
  websocketHardwareMtu(): number;
  usbAutoHostBitrateBps(): number;
  usbAutoHostHardwareMtu(): number;
  usbAutoWebUsbVendorId(): number;
  usbAutoWebUsbProductId(): number;
  usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array;
  usbAutoHostHelloFrame(): Uint8Array;
  usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array;
  usbAutoDataFrame(packet: PacketFrame): Uint8Array;
  usbAutoConfigRequestFrame(requestId: number, action: Uint8Array): Uint8Array;
  usbAutoSnapshotDecode(body: Uint8Array): UsbAutoSnapshot;
  usbAutoConfigActionSetLoRaProfile(
    frequencyHz: number,
    spreadingFactor: number,
    bandwidth: number,
    codingRate: number,
    txPowerDbm: number,
    preamble: number,
    regionCode: number,
  ): Uint8Array;
  usbAutoConfigActionResetLoRaProfile(): Uint8Array;
  usbAutoConfigActionToggleInterface(interfaceCode: number): Uint8Array;
  usbAutoConfigActionSleep(): Uint8Array;
  usbAutoConfigActionWake(): Uint8Array;
  usbAutoConfigActionAnnounce(): Uint8Array;
  usbAutoConfigActionRequestSnapshot(): Uint8Array;
};

export type RuntimeRegisterNodePageOptions = {
  appData?: Uint8Array;
};

export type PrnsRuntimeBinding = {
  registerInterface(options: RuntimeRegisterInterfaceInput): InterfaceId;
  removeInterface(options: RuntimeRemoveInterfaceInput): boolean;
  bluetoothIdentity(): Uint8Array;
  registerSingleDestination(options: RuntimeRegisterSingleDestinationOptions): DestinationHash;
  registerNodePage(options: RuntimeRegisterNodePageOptions): DestinationHash;
  announce(options: RuntimeAnnounceOptions): bigint;
  sendSinglePacket(options: RuntimeSendSinglePacketOptions): bigint;
  establishLink(options: RuntimeDestinationCommandOptions): bigint;
  requestPath(options: RuntimeDestinationCommandOptions): bigint;
  identify(options: RuntimeIdentifyOptions): bigint;
  sendLinkPacket(options: RuntimeLinkPayloadOptions): bigint;
  request(options: RuntimeRequestOptions): bigint;
  respond(options: RuntimeRespondOptions): bigint;
  resourceSegmentPlan(options: RuntimeResourcePlanInput): unknown;
  sendResourceSegment(options: RuntimeResourceSegmentInput): bigint;
  setLinkResourceStrategy(
    options: RuntimeLinkResourceStrategyOptions,
  ): bigint;
  setDestinationResourceStrategy(
    options: RuntimeDestinationResourceStrategyOptions,
  ): boolean;
  sendChannelMessage(options: RuntimeChannelMessageOptions): bigint;
  allowRequester(options: RuntimeAllowRequesterOptions): bigint;
  closeLink(options: RuntimeCloseLinkOptions): bigint;
  ingest(options: RuntimeIngestOptions): void;
  drainEvents(): unknown[];
  drainOutbound(): unknown[];
  persistedState(options: { readonly nowMs: InstantMillis }): unknown;
  restorePersistedState(
    options: BrowserPersistedState & { readonly nowMs: InstantMillis },
  ): unknown;
  snapshot(): unknown;
};

export type UsbAutoDecoderBinding = {
  feed(chunk: Uint8Array): unknown[];
};

export type BluetoothReassemblerBinding = {
  absorb(bytes: Uint8Array): Uint8Array | undefined;
};

export type InterfaceName =
  | "usb-auto"
  | "rnode"
  | "bluetooth"
  | "auto-wifi"
  | "websocket"
  | "serial"
  | "kiss"
  | "pipe";

export type RuntimeInterfaceKind =
  | "auto-usb-host"
  | "auto-usb-device"
  | "rnode"
  | "bluetooth-auto"
  | "bluetooth-peer"
  | "auto-wifi"
  | "websocket-client"
  | "websocket-server"
  | "websocket-server-peer"
  | "serial"
  | "kiss"
  | "pipe";

export type RuntimeRegisterInterfaceOptions = {
  kind: RuntimeInterfaceKind;
  channelTag: ChannelTag;
  bitrateBps?: BitrateBps;
  hardwareMtu?: HardwareMtu;
  mode?: InterfaceRoutingPolicy["mode"];
  gravity?: number;
  recursivePathRequests?: boolean;
  announcesFromInternal?: boolean;
  announcesToInternal?: boolean;
};

export type RuntimeRegisterInterfaceInput = RuntimeRegisterInterfaceOptions & {
  nowMs: InstantMillis;
};

export type RuntimeRemoveInterfaceInput = {
  interfaceId: InterfaceId;
  nowMs: InstantMillis;
};

export type RuntimeRegisterSingleDestinationOptions = {
  appName: AppName;
  aspects: readonly Aspect[];
  appData?: AppData;
  maximumRequestBytes?: number;
  requestHandlers?: readonly RequestHandlerConfig[];
};

export type RegisterSingleDestinationOptions =
  RuntimeRegisterSingleDestinationOptions;

export type RuntimeAnnounceOptions = {
  destination: DestinationHash;
  interfaceId?: InterfaceId;
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

export type RuntimeSendSinglePacketOptions = {
  destination: DestinationHash;
  payload: Uint8Array;
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

export type RuntimeCloseLinkOptions = {
  linkId: LinkId;
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

type RuntimeCommandContext = {
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

export type RuntimeDestinationCommandOptions = RuntimeCommandContext & {
  destination: DestinationHash;
};

export type RuntimeIdentifyOptions = RuntimeCommandContext & {
  linkId: LinkId;
  identity: IdentityHash;
};

export type RuntimeLinkPayloadOptions = RuntimeCommandContext & {
  linkId: LinkId;
  payload: Uint8Array;
};

export type RuntimeRequestOptions = RuntimeLinkPayloadOptions & {
  pathHash: RequestPathHash;
  timeoutMillis?: number;
  maximumResponseBytes?: number;
};

export type RuntimeRespondOptions = RuntimeLinkPayloadOptions & {
  requestId: RequestId;
  requestRttMillis: number;
};

type RuntimeResourceStrategy =
  | {
      strategy: "refuse";
    }
  | {
      strategy: "accept";
      maximumUncompressedBytes: number;
      acceptCompressed: boolean;
    };

export type RuntimeLinkResourceStrategyOptions = RuntimeCommandContext &
  RuntimeResourceStrategy & {
    linkId: LinkId;
  };

export type RuntimeDestinationResourceStrategyOptions =
  RuntimeResourceStrategy & {
    destination: DestinationHash;
  };

export type RuntimeChannelMessageOptions = RuntimeLinkPayloadOptions & {
  messageType: number;
};

export type RuntimeAllowRequesterOptions = RuntimeCommandContext & {
  destination: DestinationHash;
  pathHash: RequestPathHash;
  identity: IdentityHash;
};

export type RuntimeIngestOptions = {
  interfaceId: InterfaceId;
  bytes: PacketFrame;
  nowMs: InstantMillis;
  entropy: EntropyBytes;
};

export type AnnounceEvent = Extract<
  HostDiagnosticEvent,
  Tag<"AnnounceHeard", unknown>
>;
export type SingleDeliveryEvent = Extract<
  HostApplicationEvent,
  Tag<"SingleDelivery", unknown>
>;
export type RequestEvent = Extract<
  HostApplicationEvent,
  Tag<"Request", unknown>
>;
export type ResponseEvent = Extract<
  HostApplicationEvent,
  Tag<"Response", unknown>
>;
export type ResponseSegmentEvent = Extract<
  HostApplicationEvent,
  Tag<"ResponseSegment", unknown>
>;
export type ResourceAvailableEvent = Extract<
  HostApplicationEvent,
  Tag<"ResourceAvailable", unknown>
>;
export type ResourceSegmentEvent = Extract<
  HostApplicationEvent,
  Tag<"ResourceSegment", unknown>
>;
export type ChannelMessageEvent = Extract<
  HostApplicationEvent,
  Tag<"ChannelMessage", unknown>
>;

type CommandSettledEvent = Tag<
  "CommandSettled",
  {
    readonly commandId: CommandId;
    readonly settlement?: CommandSettlement;
  }
>;

type PendingCommand =
  | Tag<"HostCommand", { readonly command: HostCommand }>
  | Tag<"ResourceSegment">;

export type RouteEvent = Extract<
  HostDiagnosticEvent,
  Tag<
    "RouteExpired" | "RouteEvicted" | "RouteInterfaceGone" | "RouteDropped",
    unknown
  >
>;
export type DiagnosticsDroppedEvent = Extract<
  HostDiagnosticEvent,
  Tag<"DiagnosticsDropped", unknown>
>;
export type LinkEvent = Extract<
  HostDiagnosticEvent,
  Tag<
    | "LinkEstablished"
    | "PeerIdentified"
    | "LinkClosed"
    | "LinkInterfaceMismatch",
    unknown
  >
>;
export type ResourceDiagnosticEvent = Extract<
  HostDiagnosticEvent,
  Tag<
    | "ResourceAssembled"
    | "ResourceFailed"
    | "ResourceSendProgress",
    unknown
  >
>;
export type RuntimeDiagnosticEvent = Extract<
  HostDiagnosticEvent,
  Tag<
    "SelfRatchetRotated" | "AnnounceHeldDropped" | "Delivered",
    unknown
  >
>;

export type PrnsApplicationEvent = HostApplicationEvent;
export type PrnsDiagnosticEvent = HostDiagnosticEvent;
export type PrnsEvent = PrnsApplicationEvent | PrnsDiagnosticEvent;
type ParsedPrnsEvent =
  | Tag<"Application", PrnsApplicationEvent>
  | Tag<"Diagnostic", Exclude<PrnsDiagnosticEvent, DiagnosticsDroppedEvent>>
  | Tag<
      "CommandResponse",
      {
        readonly commandId: CommandId;
        readonly event: ResponseEvent;
      }
    >
  | Tag<
      "CommandResponseSegment",
      {
        readonly commandId: CommandId;
        readonly event: ResponseSegmentEvent;
      }
    >
  | CommandSettledEvent;

type RawEventType =
  | "announce"
  | "selfRatchetRotated"
  | "announceHeldDropped"
  | "commandSettled"
  | "linkEstablished"
  | "peerIdentified"
  | "request"
  | "response"
  | "responseSegment"
  | "channelMessage"
  | "singleDelivery"
  | "delivered"
  | "linkClosed"
  | "linkInterfaceMismatch"
  | "resourceReceived"
  | "resourceFailed"
  | "resourceNeedsDecompression"
  | "resourceSegment"
  | "resourceAssembled"
  | "routeExpired"
  | "routeEvicted"
  | "routeInterfaceGone"
  | "routeDropped";

type RawEvent = {
  [Name in RawEventType]: Tag<Name, Record<string, unknown>>;
}[RawEventType];

const RAW_EVENT_TYPES: ReadonlySet<string> = new Set<RawEventType>([
  "announce",
  "selfRatchetRotated",
  "announceHeldDropped",
  "commandSettled",
  "linkEstablished",
  "peerIdentified",
  "request",
  "response",
  "responseSegment",
  "channelMessage",
  "singleDelivery",
  "delivered",
  "linkClosed",
  "linkInterfaceMismatch",
  "resourceReceived",
  "resourceFailed",
  "resourceNeedsDecompression",
  "resourceSegment",
  "resourceAssembled",
  "routeExpired",
  "routeEvicted",
  "routeInterfaceGone",
  "routeDropped",
]);

export type FanTarget =
  | Tag<"All">
  | Tag<"Only", InterfaceId>
  | Tag<"AllExcept", InterfaceId>;

export type OutboundTarget =
  | Tag<"Interface", InterfaceId>
  | Tag<
      "Broadcast",
      {
        readonly supervisorKind: RuntimeInterfaceKind;
        readonly fan: FanTarget;
      }
    >;

export type PrnsOutboundFrame = {
  type: "frame" | "announce";
  target: OutboundTarget;
  hops?: HopCount;
  bytes: PacketFrame;
};

export type InterfaceSnapshot = {
  id: InterfaceId;
  kind: string;
  bitrateBps?: BitrateBps;
  hardwareMtu?: HardwareMtu;
  routes: number;
  links: number;
  transportedLinks: number;
};

export type PrnsSnapshot = {
  type: "snapshot";
  revision: bigint;
  ingestedPackets: number;
  ingestedCommands: number;
  routes: number;
  scheduledAnnounces: number;
  interfaces: InterfaceSnapshot[];
  activeLinkCount: number;
  routeSnapshots: StableRouteSnapshot[];
  destinationIdentities: StableDestinationIdentitySnapshot[];
};

export type IdentityStore = {
  load(expectedLength: number): Promise<IdentityLoadOutcome>;
  save(secretKey: IdentitySecretKey): Promise<IdentitySaveOutcome>;
};

export type StableIdentityLoadOutcome =
  | Tag<"Loaded", Uint8Array>
  | Tag<"Missing">
  | StableIdentityStoreFailure;

export type StableIdentitySaveOutcome =
  | Tag<"Saved">
  | StableIdentityStoreFailure;

export type StableIdentityStore = {
  load(expectedLength: number): Promise<StableIdentityLoadOutcome>;
  save(identity: Uint8Array): Promise<StableIdentitySaveOutcome>;
};

export type BrowserPersistedRatchet = {
  readonly destination: DestinationHash;
  readonly sealed: Uint8Array;
};

export type BrowserPersistedState = {
  readonly type: "persistedState";
  readonly persistenceVersion: number;
  readonly takenAtMillis: InstantMillis;
  readonly routingTable: Uint8Array;
  readonly tunnels: Uint8Array;
  readonly destinationIdentities: Uint8Array;
  readonly ratchets: readonly BrowserPersistedRatchet[];
};

export type PersistenceLoadOutcome =
  | Tag<"Loaded", BrowserPersistedState>
  | Tag<"Missing">
  | PersistenceStoreFailure;

export type PersistenceSaveOutcome =
  | Tag<"Saved">
  | PersistenceStoreFailure;

export type BrowserPersistenceStore = {
  load(): Promise<PersistenceLoadOutcome>;
  save(state: BrowserPersistedState): Promise<PersistenceSaveOutcome>;
};

type BrowserPersistenceRestoreReport = {
  readonly routes: number;
  readonly destinationIdentities: number;
  readonly tunnels: number;
  readonly ratchets: number;
  readonly refused: number;
  readonly dropped: number;
};

export const BROWSER_PERSISTENCE_VERSION = 1;

type HostGlobal = typeof globalThis & {
  crypto?: {
    getRandomValues<T extends Uint8Array>(array: T): T;
  };
  navigator?: {
    bluetooth?: BrowserBluetooth;
    usb?: BrowserUsb;
  };
  localStorage?: {
    getItem(key: string): string | null;
    setItem(key: string, value: string): void;
  };
  btoa?: (data: string) => string;
  atob?: (data: string) => string;
  WebSocket?: typeof WebSocket;
};

type BrowserBluetooth = {
  requestDevice(options: BrowserBluetoothRequestOptions): Promise<BrowserBluetoothDevice>;
};

type BrowserBluetoothRequestOptions = {
  filters: readonly BrowserBluetoothRequestFilter[];
  optionalServices?: readonly string[];
};

type BrowserBluetoothRequestFilter = {
  services: readonly string[];
};

type BrowserBluetoothDevice = {
  readonly gatt?: BrowserBluetoothRemoteGattServer;
};

type BrowserBluetoothRemoteGattServer = {
  connect(): Promise<BrowserBluetoothRemoteGattServer>;
  disconnect(): void;
  getPrimaryService(service: string): Promise<BrowserBluetoothRemoteGattService>;
};

type BrowserBluetoothRemoteGattService = {
  getCharacteristic(characteristic: string): Promise<BrowserBluetoothRemoteGattCharacteristic>;
};

type BrowserBluetoothRemoteGattCharacteristic = EventTarget & {
  readonly value?: DataView;
  startNotifications(): Promise<BrowserBluetoothRemoteGattCharacteristic>;
  writeValue?(value: BufferSource): Promise<void>;
  writeValueWithResponse?(value: BufferSource): Promise<void>;
  writeValueWithoutResponse?(value: BufferSource): Promise<void>;
};

type BrowserBluetoothCharacteristicEvent = Event & {
  target: BrowserBluetoothRemoteGattCharacteristic | null;
};

type BrowserUsb = {
  requestDevice(options: BrowserUsbRequestOptions): Promise<BrowserUsbDevice>;
};

type BrowserUsbRequestOptions = {
  filters: readonly BrowserUsbDeviceFilter[];
};

type BrowserUsbDeviceFilter = {
  vendorId?: number;
  productId?: number;
  classCode?: number;
  subclassCode?: number;
  protocolCode?: number;
  serialNumber?: string;
};

type BrowserUsbDevice = {
  readonly vendorId: number;
  readonly productId: number;
  readonly manufacturerName?: string;
  readonly productName?: string;
  readonly serialNumber?: string;
  readonly configurations: readonly BrowserUsbConfiguration[];
  readonly configuration?: BrowserUsbConfiguration | null;
  open(): Promise<void>;
  close(): Promise<void>;
  selectConfiguration(configurationValue: number): Promise<void>;
  claimInterface(interfaceNumber: number): Promise<void>;
  releaseInterface(interfaceNumber: number): Promise<void>;
  selectAlternateInterface?(
    interfaceNumber: number,
    alternateSetting: number,
  ): Promise<void>;
  transferIn(endpointNumber: number, length: number): Promise<BrowserUsbInTransferResult>;
  transferOut(
    endpointNumber: number,
    data: BufferSource,
  ): Promise<BrowserUsbOutTransferResult>;
};

type BrowserUsbConfiguration = {
  readonly configurationValue: number;
  readonly interfaces: readonly BrowserUsbInterface[];
};

type BrowserUsbInterface = {
  readonly interfaceNumber: number;
  readonly alternates: readonly BrowserUsbAlternateInterface[];
  readonly claimed?: boolean;
};

type BrowserUsbAlternateInterface = {
  readonly alternateSetting: number;
  readonly interfaceClass?: number;
  readonly interfaceSubclass?: number;
  readonly interfaceProtocol?: number;
  readonly endpoints: readonly BrowserUsbEndpoint[];
};

type BrowserUsbEndpoint = {
  readonly endpointNumber: number;
  readonly direction: "in" | "out";
  readonly type: "bulk" | "interrupt" | "isochronous";
  readonly packetSize: number;
};

type BrowserUsbInTransferResult = {
  readonly data?: DataView;
  readonly status: "ok" | "stall" | "babble";
};

type BrowserUsbOutTransferResult = {
  readonly bytesWritten: number;
  readonly status: "ok" | "stall";
};

type UsbAutoInboundMessage =
  | Tag<"Hello">
  | Tag<"HelloAck", Uint8Array>
  | Tag<"Data", Uint8Array>
  | Tag<"ConfigRequest", { requestId: number; action: Uint8Array }>
  | Tag<"ConfigResponse", { requestId: number; result: UsbAutoConfigResult; detail: Uint8Array }>
  | Tag<"Snapshot", { schemaVersion: number; body: Uint8Array }>;

export type UsbAutoConfigResult =
  | "ok"
  | "applyFailed"
  | "profileNotSaved"
  | "rejected"
  | "badPayload";

export type UsbAutoInterfaceKind = "lora" | "usb" | "ble";

export type UsbAutoConnectionState =
  | "initializing"
  | "connected"
  | "degraded"
  | "reconnecting"
  | "failed"
  | "disconnected"
  | "disabled"
  | "unknown";

export type UsbAutoPersistenceState = "durable" | "deferred" | "failed";

export type UsbAutoInterfaceStatus = {
  enabled: boolean;
  connection: UsbAutoConnectionState;
  rxBytes: number;
  txBytes: number;
  airtime?: { shortPerMille: number; longPerMille: number };
  transferRates?: { rxBps: number; txBps: number };
};

export type UsbAutoRadioProfile = {
  frequencyHz: number;
  spreadingFactor: number;
  bandwidth: number;
  codingRate: number;
  txPowerDbm: number;
  preamble: number;
  regionCode: number;
  region: string;
};

export type UsbAutoInterfaceCount = {
  kind: UsbAutoInterfaceKind;
  destinations: number;
  links: number;
  transportedLinks: number;
};

export type UsbAutoSnapshotSection =
  | { type: "deviceInfo"; version: string }
  | { type: "persistence"; state: UsbAutoPersistenceState }
  | { type: "loraStatus"; status: UsbAutoInterfaceStatus }
  | { type: "usbStatus"; status: UsbAutoInterfaceStatus }
  | {
      type: "bleStatus";
      status: UsbAutoInterfaceStatus;
      failureReason: string;
    }
  | {
      type: "bleRecovery";
      ingressPressure: number;
      setupFailures: number;
      transportClosures: number;
      egressPressureEvents: number;
      memberCount: number;
    }
  | {
      type: "loraSpectrum";
      channelBusyPerMille: number;
      noiseFloorDbm?: number;
      ccaThresholdDbm?: number;
      deferrals: number;
      falsePreambles: number;
      contentionTimeouts: number;
      dutyHolds: number;
      dutyTimeouts: number;
      radioRecoveries: number;
    }
  | { type: "radioProfile"; profile: UsbAutoRadioProfile }
  | { type: "interfaceCounts"; counts: UsbAutoInterfaceCount[] };

export type UsbAutoSnapshot = {
  sections: UsbAutoSnapshotSection[];
};

type BluetoothControl =
  | Tag<"Hello", Uint8Array>
  | Tag<"Welcome", Uint8Array>
  | Tag<"Close", string>;

type SessionWriteOutcome = Tag<"Written"> | InterfaceSessionFailure;
type SessionHandleOutcome = Tag<"Handled"> | InterfaceSessionFailure;
type UsbReadOutcome =
  | Tag<"Read", Uint8Array | undefined>
  | InterfaceSessionFailure;
type WebUsbOpenOutcome =
  | Tag<"Opened", WebUsbAutoTransport>
  | PermissionDenied<"usb-auto">
  | Cancelled<"usb-auto">
  | UnsupportedDevice<"usb-auto">
  | ConnectionFailed<"usb-auto">;
type UsbConfigurationOutcome =
  | Tag<"Configured", BrowserUsbConfiguration>
  | UnsupportedDevice<"usb-auto">;
type InterfaceRegistrationOutcome<Name extends InterfaceName> =
  | Tag<"Registered", InterfaceId>
  | AlreadyActive<Name>
  | RuntimeRejected;
type HostedInterfaceRegistration<Name extends InterfaceName> =
  RuntimeRegisterInterfaceOptions & {
    readonly interfaceName: Name;
    readonly supervisorKind?: RuntimeInterfaceKind;
    readonly contractKind?: InterfaceKind;
  };
type RuntimeInterfaceInspection = {
  readonly id: InterfaceId;
  readonly name: InterfaceName;
  readonly kind?: InterfaceKind;
  readonly rxBytes: number;
  readonly txBytes: number;
};
type InterfaceDetachOutcome = Tag<"Detached"> | RuntimeRejected;
type RuntimeReadyOutcome = Tag<"Ready"> | RuntimeRejected;
type RuntimeIngestOutcome = Tag<"Accepted"> | EntropyFailure | RuntimeRejected;
type OutboundTakeOutcome =
  | Tag<"Outbound", readonly PrnsOutboundFrame[]>
  | Extract<InterfaceSessionFailure, Tag<"OutboundQueueFull", unknown>>
  | RuntimeRejected;
type BluetoothStartOutcome = Tag<"Started"> | BluetoothConnectFailure;
type BluetoothHandleOutcome =
  | SessionHandleOutcome
  | AlreadyActive<"bluetooth">;
type IdentityGenerationOutcome =
  | Tag<"Generated", IdentitySecretKey>
  | HostApiUnavailable<"Crypto">
  | Tag<"EntropySourceFailed", { readonly detail: string }>;
export type BleIdentityAvailability =
  | Tag<"Available", BleIdentity>
  | StableIdentityUnavailable<"bluetooth">;
export type BleIdentityValidationOutcome =
  | Tag<"ValidBleIdentity", BleIdentity>
  | Tag<"InvalidBleIdentity", { readonly actualLength: number }>;
type Available<Host, Api extends HostApi> =
  | Tag<"Available", Host>
  | HostApiUnavailable<Api>;
type UsbStageOutcome<Value> =
  | Tag<"Completed", Value>
  | PermissionDenied<"usb-auto">
  | Cancelled<"usb-auto">
  | ConnectionFailed<"usb-auto">;
type BluetoothStageOutcome<Value> =
  | Tag<"Completed", Value>
  | PermissionDenied<"bluetooth">
  | Cancelled<"bluetooth">
  | ConnectionFailed<"bluetooth">;
type WebSocketOpenOutcome =
  | Tag<"Opened", WebSocket>
  | HostApiUnavailable<"WebSocket">
  | PermissionDenied<"websocket">
  | Cancelled<"websocket">
  | ConnectTimedOut<"websocket">
  | ConnectionFailed<"websocket">;
type WebSocketDecodeOutcome =
  | Tag<"Decoded", Uint8Array>
  | Extract<InterfaceSessionFailure, Tag<"UnsupportedFrame", unknown>>
  | Extract<InterfaceSessionFailure, Tag<"FrameTooLarge", unknown>>
  | Extract<InterfaceSessionFailure, Tag<"TransferFailed", unknown>>;
type CanonicalWebSocketOutcome =
  | Tag<"Canonical", string>
  | InvalidTarget<"websocket">;
type CharacteristicBytesOutcome =
  | Tag<"Decoded", Uint8Array>
  | Extract<InterfaceSessionFailure, Tag<"ProtocolViolation", unknown>>;
type RuntimeOutboundDrainOutcome =
  | Tag<"Drained", readonly PrnsOutboundFrame[]>
  | RuntimeRejected;

const USB_AUTO_PROBE_INTERVAL_MS = 500;
const USB_AUTO_OUTBOUND_POLL_MS = 25;
const WEBUSB_MIN_TRANSFER_BYTES = 512;
const BLUETOOTH_HANDSHAKE_TIMEOUT_MS = 10_000;
const BLUETOOTH_OUTBOUND_POLL_MS = 25;
const WEBSOCKET_CONNECT_TIMEOUT_MS = 10_000;
const WEBSOCKET_OUTBOUND_POLL_MS = 25;
const WEBSOCKET_BUFFER_POLL_MS = 4;
const WEBSOCKET_MIN_BUFFER_LIMIT = 1024 * 1024;
const WEBSOCKET_CONNECTING = 0;
const WEBSOCKET_OPEN = 1;
const INTERFACE_OUTBOUND_QUEUE_DEPTH = 64;
let nextBrowserUsbAutoTag = 0;
const LINUX_WEBUSB_SETUP_HINT =
  "On Linux, run ./tools/prns device webusb install from the Prns repo root, " +
  "then unplug/replug the device and restart the browser. If this is Snap Chromium, " +
  "also run sudo snap connect chromium:raw-usb or use a non-Snap Chrome/Chromium build.";

export class BrowserLocalStorageIdentityStore implements IdentityStore {
  #key: string;

  constructor(key: string = "prns.identity.v1") {
    this.#key = key;
  }

  async load(expectedLength: number): Promise<IdentityLoadOutcome> {
    let encoded: string | null;
    try {
      const storage = hostGlobal().localStorage;
      if (!storage) {
        return Tag("HostApiUnavailable", { api: "LocalStorage" });
      }
      if (!hostGlobal().atob) {
        return Tag("HostApiUnavailable", { api: "Base64Decoder" });
      }
      encoded = storage.getItem(this.#key);
    } catch (error) {
      return Tag("IdentityStoreFailed", {
        operation: "Load",
        detail: describeHostError(error),
      });
    }
    if (encoded === null) {
      return Tag("Missing");
    }
    try {
      return Tag(
        "Loaded",
        identitySecretKey(decodeBase64(encoded), expectedLength),
      );
    } catch (error) {
      return Tag("StoredIdentityInvalid", {
        detail: describeHostError(error),
      });
    }
  }

  async save(secretKey: IdentitySecretKey): Promise<IdentitySaveOutcome> {
    try {
      const storage = hostGlobal().localStorage;
      if (!storage) {
        return Tag("HostApiUnavailable", { api: "LocalStorage" });
      }
      if (!hostGlobal().btoa) {
        return Tag("HostApiUnavailable", { api: "Base64Encoder" });
      }
      storage.setItem(this.#key, encodeBase64(secretKey));
      return Tag("Saved");
    } catch (error) {
      return Tag("IdentityStoreFailed", {
        operation: "Save",
        detail: describeHostError(error),
      });
    }
  }
}

export class BrowserLocalStorageBleIdentityStore implements StableIdentityStore {
  #key: string;

  constructor(key: string = "prns.ble-identity.v1") {
    this.#key = key;
  }

  async load(expectedLength: number): Promise<StableIdentityLoadOutcome> {
    try {
      const storage = hostGlobal().localStorage;
      if (!storage) {
        return Tag("HostApiUnavailable", { api: "LocalStorage" });
      }
      if (!hostGlobal().atob) {
        return Tag("HostApiUnavailable", { api: "Base64Decoder" });
      }
      const encoded = storage.getItem(this.#key);
      if (encoded === null) {
        return Tag("Missing");
      }
      const bytes = decodeBase64(encoded);
      if (bytes.length !== expectedLength) {
        return Tag("StoredStableIdentityInvalid", {
          detail: `stored Bluetooth LE identity has ${bytes.length} bytes; expected ${expectedLength}`,
        });
      }
      return Tag("Loaded", bytes);
    } catch (error) {
      return Tag("StableIdentityStoreFailed", {
        operation: "Load",
        detail: describeHostError(error),
      });
    }
  }

  async save(identity: Uint8Array): Promise<StableIdentitySaveOutcome> {
    try {
      const storage = hostGlobal().localStorage;
      if (!storage) {
        return Tag("HostApiUnavailable", { api: "LocalStorage" });
      }
      if (!hostGlobal().btoa) {
        return Tag("HostApiUnavailable", { api: "Base64Encoder" });
      }
      storage.setItem(this.#key, encodeBase64(identity));
      return Tag("Saved");
    } catch (error) {
      return Tag("StableIdentityStoreFailed", {
        operation: "Save",
        detail: describeHostError(error),
      });
    }
  }
}

export class BrowserLocalStoragePersistenceStore
  implements BrowserPersistenceStore
{
  #key: string;

  constructor(key: string = "prns.state.v1") {
    this.#key = key;
  }

  async load(): Promise<PersistenceLoadOutcome> {
    let encoded: string | null;
    try {
      const storage = hostGlobal().localStorage;
      if (!storage) {
        return Tag("HostApiUnavailable", { api: "LocalStorage" });
      }
      if (!hostGlobal().atob) {
        return Tag("HostApiUnavailable", { api: "Base64Decoder" });
      }
      encoded = storage.getItem(this.#key);
    } catch (error) {
      return Tag("PersistenceStoreFailed", {
        operation: "Load",
        detail: describeHostError(error),
      });
    }
    if (encoded === null) {
      return Tag("Missing");
    }
    try {
      return Tag("Loaded", decodeBrowserPersistedState(encoded));
    } catch (error) {
      return Tag("StoredPersistenceInvalid", {
        detail: describeHostError(error),
      });
    }
  }

  async save(state: BrowserPersistedState): Promise<PersistenceSaveOutcome> {
    try {
      const storage = hostGlobal().localStorage;
      if (!storage) {
        return Tag("HostApiUnavailable", { api: "LocalStorage" });
      }
      if (!hostGlobal().btoa) {
        return Tag("HostApiUnavailable", { api: "Base64Encoder" });
      }
      storage.setItem(this.#key, encodeBrowserPersistedState(state));
      return Tag("Saved");
    } catch (error) {
      return Tag("PersistenceStoreFailed", {
        operation: "Save",
        detail: describeHostError(error),
      });
    }
  }
}

export type EntropySource = (length: number) => EntropyOutcome;

export type PrnsOptions = {
  wasm?: PrnsWasmModule;
  resourceCompressionModuleUrl?: URL;
  identityStore?: IdentityStore;
  bleIdentityStore?: StableIdentityStore;
  persistenceStore?: BrowserPersistenceStore;
  entropy?: EntropySource;
  now?: () => InstantMillis;
  limits?: HostLimits;
};

export function persistentBrowser(root: string = "prns"): PrnsOptions {
  const selected = root.trim();
  if (selected.length === 0) {
    throw new PrnsValidationError(
      "invalid-component",
      "browser persistence root must not be empty",
    );
  }
  return {
    identityStore: new BrowserLocalStorageIdentityStore(
      `${selected}.identity.v1`,
    ),
    bleIdentityStore: new BrowserLocalStorageBleIdentityStore(
      `${selected}.ble-identity.v1`,
    ),
    persistenceStore: new BrowserLocalStoragePersistenceStore(
      `${selected}.state.v1`,
    ),
  };
}

export type InterfaceSession = {
  readonly name: InterfaceName;
  readonly interfaceId: InterfaceId;
  readonly status: InterfaceSessionStatus;
  close(): Promise<InterfaceCloseOutcome>;
};

export type UsbAutoSession = InterfaceSession & {
  readonly name: "usb-auto";
};

export type BluetoothSession = InterfaceSession & {
  readonly name: "bluetooth";
};

export type WebSocketSession = InterfaceSession & {
  readonly name: "websocket";
  readonly url: string;
};

export type UsbAutoDeviceFilter = {
  readonly vendorId?: number;
  readonly productId?: number;
  readonly serialNumber?: string;
};

export type UsbAutoConnectOptions = {
  readonly filters?: readonly UsbAutoDeviceFilter[];
};

export type WebSocketConnectOptions = {
  readonly protocols?: string | readonly string[];
  readonly channelTag?: ChannelTag;
  readonly bitrateBps?: BitrateBps;
  readonly hardwareMtu?: HardwareMtu;
  readonly routing?: InterfaceRoutingPolicy;
};

export class PrnsInterfaces {
  readonly usbAuto: UsbAutoInterface;
  readonly rnode: RNodeInterface;
  readonly bluetooth: BluetoothInterface;
  readonly autoWifi: AutoWifiInterface;
  readonly webSocket: WebSocketInterface;

  constructor(host: RuntimeHost) {
    this.usbAuto = new UsbAutoInterface(host);
    this.rnode = new RNodeInterface(host);
    this.bluetooth = new BluetoothInterface(host);
    this.autoWifi = new AutoWifiInterface(host);
    this.webSocket = new WebSocketInterface(host);
  }
}

export class UsbAutoInterface {
  readonly name = "usb-auto" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(
    options: UsbAutoConnectOptions = {},
  ): Promise<UsbAutoConnectOutcome> {
    const ready = this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    const available = requireWebUsb();
    if (available.tag !== "Available") {
      return available;
    }
    let transport: WebUsbAutoTransport | undefined;
    let interfaceId: InterfaceId | undefined;
    let stage: InterfaceConnectStage = "DeviceSelection";
    try {
      const requested = await usbStage("DeviceSelection", "request device", () =>
        available.data.requestDevice({
          filters: options.filters ?? this.#host.defaultUsbAutoFilters(),
        }),
      );
      if (requested.tag !== "Completed") {
        return requested;
      }
      stage = "TransportOpen";
      const opened = await WebUsbAutoTransport.open(requested.data);
      if (opened.tag !== "Opened") {
        return opened;
      }
      transport = opened.data;
      stage = "RuntimeRegistration";
      const registered = this.#host.registerInterface({
        interfaceName: "usb-auto",
        kind: "auto-usb-host",
        channelTag: browserUsbAutoChannelTag(requested.data),
        bitrateBps: this.#host.usbAutoHostBitrateBps(),
        hardwareMtu: this.#host.usbAutoHostHardwareMtu(),
      });
      if (registered.tag !== "Registered") {
        await transport.close();
        return registered;
      }
      interfaceId = registered.data;
      stage = "Handshake";
      const session = new BrowserUsbAutoSession(this.#host, transport, interfaceId);
      session.start();
      return Tag("Connected", session);
    } catch (error) {
      if (interfaceId) {
        this.#host.deactivateInterface(interfaceId);
      }
      await transport?.close();
      return connectFailure("usb-auto", stage, error);
    }
  }
}

class BrowserUsbAutoSession implements UsbAutoSession {
  readonly name = "usb-auto" as const;
  readonly interfaceId: InterfaceId;

  readonly #host: RuntimeHost;
  readonly #transport: WebUsbAutoTransport;
  readonly #decoder: UsbAutoDecoderBinding;
  readonly #nodeTag: Uint8Array;
  #writeQueue: Promise<SessionWriteOutcome> = Promise.resolve(Tag("Written"));
  #closed = false;
  #confirmed = false;
  #status: InterfaceSessionStatus = Tag("Negotiating");

  constructor(
    host: RuntimeHost,
    transport: WebUsbAutoTransport,
    interfaceId: InterfaceId,
  ) {
    this.#host = host;
    this.#transport = transport;
    this.interfaceId = interfaceId;
    this.#decoder = host.createUsbAutoDecoder();
    this.#nodeTag = host.usbAutoNodeTagFor(interfaceId);
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  start(): void {
    void this.#readLoop();
    void this.#probeLoop();
    void this.#outboundLoop();
  }

  async close(): Promise<InterfaceCloseOutcome> {
    if (this.#closed) {
      return closedSessionOutcome(this.#status);
    }
    this.#closed = true;
    const causes: InterfaceCleanupFailure[] = [];
    const detached = this.#host.deactivateInterface(this.interfaceId);
    if (detached.tag !== "Detached") {
      causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
    }
    const pendingWrite = await this.#writeQueue;
    if (pendingWrite.tag !== "Written") {
      causes.push(
        Tag("TransportCloseFailed", {
          detail: describeInterfaceSessionFailure(pendingWrite),
        }),
      );
    }
    causes.push(...(await this.#transport.close()));
    if (hasCleanupFailures(causes)) {
      const failed = closeFailed(causes);
      this.#status = Tag("Failed", failed);
      return failed;
    }
    this.#status = Tag("Closed");
    return Tag("Closed");
  }

  async #readLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const read = await this.#transport.read();
        if (read.tag !== "Read") {
          await this.#fail(read);
          return;
        }
        const chunk = read.data;
        if (!chunk) {
          break;
        }
        if (chunk.length === 0) {
          continue;
        }
        let messages: unknown[];
        try {
          messages = this.#decoder.feed(chunk);
        } catch (error) {
          await this.#fail(
            Tag("ProtocolViolation", {
              protocol: "UsbAuto",
              detail: describeHostError(error),
            }),
          );
          return;
        }
        for (const raw of messages) {
          let message: UsbAutoInboundMessage;
          try {
            message = parseUsbAutoMessage(raw);
          } catch (error) {
            await this.#fail(
              Tag("ProtocolViolation", {
                protocol: "UsbAuto",
                detail: describeHostError(error),
              }),
            );
            return;
          }
          const handled = await this.#handleInbound(message);
          if (handled.tag !== "Handled") {
            await this.#fail(handled);
            return;
          }
        }
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    } finally {
      if (!this.#closed) {
        await this.close();
      }
    }
  }

  async #probeLoop(): Promise<void> {
    try {
      while (!this.#closed && !this.#confirmed) {
        const written = await this.#writeFrame(this.#host.usbAutoHostHelloFrame());
        if (written.tag !== "Written") {
          await this.#fail(written);
          return;
        }
        await delay(USB_AUTO_PROBE_INTERVAL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        if (this.#confirmed) {
          const outbound = this.#host.takeOutboundFor(this.interfaceId);
          if (outbound.tag !== "Outbound") {
            await this.#fail(outbound);
            return;
          }
          for (const frame of outbound.data) {
            const written = await this.#writeFrame(
              this.#host.usbAutoDataFrame(frame.bytes),
            );
            if (written.tag !== "Written") {
              await this.#fail(written);
              return;
            }
          }
        }
        await delay(USB_AUTO_OUTBOUND_POLL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #handleInbound(message: UsbAutoInboundMessage): Promise<SessionHandleOutcome> {
    return match_into<Promise<SessionHandleOutcome>>().from(message, {
      Hello: async () => {
        const written = await this.#writeFrame(
          this.#host.usbAutoHostHelloAckFrame(this.#nodeTag),
        );
        if (written.tag !== "Written") {
          return written;
        }
        this.#confirmPeer();
        return Tag("Handled");
      },
      HelloAck: async () => {
        this.#confirmPeer();
        return Tag("Handled");
      },
      Data: async (bytes) => {
        if (this.#confirmed && bytes.length > 0) {
          const ingested = this.#host.ingest(
            this.interfaceId,
            packetFrame(bytes),
          );
          return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
        }
        return Tag("Handled");
      },
      // The Reticulum-data session ignores config-lane traffic. The headless
      // config session (`BrowserUsbAutoConfigSession`) owns those messages.
      ConfigRequest: async () => Tag("Handled"),
      ConfigResponse: async () => Tag("Handled"),
      Snapshot: async () => Tag("Handled"),
    });
  }

  #confirmPeer(): void {
    this.#confirmed = true;
    this.#status = Tag("Active");
  }

  async #fail(sessionFailure: InterfaceSessionFailure): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#status = Tag("Failed", sessionFailure);
    this.#closed = true;
    this.#host.deactivateInterface(this.interfaceId);
    await this.#writeQueue;
    await this.#transport.close();
  }

  async #writeFrame(frame: Uint8Array): Promise<SessionWriteOutcome> {
    if (this.#closed) {
      return Tag("Written");
    }
    const write = this.#writeQueue
      .then(async (previous): Promise<SessionWriteOutcome> => {
        if (previous.tag !== "Written" || this.#closed) {
          return previous;
        }
        return this.#transport.write(frame);
      })
      .catch((error: unknown) => unexpectedSessionFailure(error));
    this.#writeQueue = write;
    return write;
  }
}

class WebUsbAutoTransport {
  readonly #device: BrowserUsbDevice;
  readonly #interfaceNumber: number;
  readonly #inEndpoint: BrowserUsbEndpoint;
  readonly #outEndpoint: BrowserUsbEndpoint;
  #closed = false;

  private constructor(
    device: BrowserUsbDevice,
    interfaceNumber: number,
    inEndpoint: BrowserUsbEndpoint,
    outEndpoint: BrowserUsbEndpoint,
  ) {
    this.#device = device;
    this.#interfaceNumber = interfaceNumber;
    this.#inEndpoint = inEndpoint;
    this.#outEndpoint = outEndpoint;
  }

  static async open(device: BrowserUsbDevice): Promise<WebUsbOpenOutcome> {
    const opened = await usbStage("TransportOpen", "open selected device", () =>
      device.open(),
    );
    if (opened.tag !== "Completed") {
      return opened;
    }
    const configured = firstUsbConfiguration(device);
    if (configured.tag !== "Configured") {
      await closeUsbDevice(device);
      return configured;
    }
    const configuration = device.configuration ?? configured.data;
    if (!device.configuration) {
      const selected = await usbStage(
        "TransportOpen",
        `select configuration ${configuration.configurationValue}`,
        () => device.selectConfiguration(configuration.configurationValue),
      );
      if (selected.tag !== "Completed") {
        await closeUsbDevice(device);
        return selected;
      }
    }
    const selectedConfiguration = device.configuration ?? configured.data;
    const endpoints = findWebUsbEndpointPair(selectedConfiguration);
    if (!endpoints) {
      await closeUsbDevice(device);
      return Tag("UnsupportedDevice", {
        interface: "usb-auto",
        capability: "usable IN/OUT endpoint pair",
      });
    }
    const claimed = await usbStage(
      "TransportOpen",
      `claim interface ${endpoints.interfaceNumber}`,
      () => device.claimInterface(endpoints.interfaceNumber),
    );
    if (claimed.tag !== "Completed") {
      await closeUsbDevice(device);
      return claimed;
    }
    if (
      endpoints.alternate.alternateSetting !== 0 &&
      device.selectAlternateInterface
    ) {
      const selected = await usbStage(
        "TransportOpen",
        `select alternate ${endpoints.alternate.alternateSetting} ` +
          `on interface ${endpoints.interfaceNumber}`,
        () =>
          device.selectAlternateInterface!(
            endpoints.interfaceNumber,
            endpoints.alternate.alternateSetting,
          ),
      );
      if (selected.tag !== "Completed") {
        await closeUsbDevice(device);
        return selected;
      }
    }
    return Tag(
      "Opened",
      new WebUsbAutoTransport(
        device,
        endpoints.interfaceNumber,
        endpoints.inEndpoint,
        endpoints.outEndpoint,
      ),
    );
  }

  async read(): Promise<UsbReadOutcome> {
    if (this.#closed) {
      return Tag("Read", undefined);
    }
    try {
      const length = Math.max(this.#inEndpoint.packetSize, WEBUSB_MIN_TRANSFER_BYTES);
      const result = await this.#device.transferIn(
        this.#inEndpoint.endpointNumber,
        length,
      );
      if (result.status !== "ok") {
        return Tag("TransferFailed", {
          direction: "Inbound",
          detail: `USB transfer status ${result.status}`,
        });
      }
      const data = result.data;
      if (!data) {
        return Tag("Read", new Uint8Array());
      }
      return Tag(
        "Read",
        new Uint8Array(
          data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength),
        ),
      );
    } catch (error) {
      return Tag("TransferFailed", {
        direction: "Inbound",
        detail: describeHostError(error),
      });
    }
  }

  async write(bytes: Uint8Array): Promise<SessionWriteOutcome> {
    if (this.#closed || bytes.length === 0) {
      return Tag("Written");
    }
    try {
      const result = await this.#device.transferOut(
        this.#outEndpoint.endpointNumber,
        arrayBufferForUsb(bytes),
      );
      if (result.status !== "ok" || result.bytesWritten !== bytes.length) {
        return Tag("TransferFailed", {
          direction: "Outbound",
          detail: `wrote ${result.bytesWritten}/${bytes.length} bytes with status ${result.status}`,
        });
      }
      return Tag("Written");
    } catch (error) {
      return Tag("TransferFailed", {
        direction: "Outbound",
        detail: describeHostError(error),
      });
    }
  }

  async close(): Promise<InterfaceCleanupFailure[]> {
    if (this.#closed) {
      return [];
    }
    this.#closed = true;
    const failures: InterfaceCleanupFailure[] = [];
    try {
      await this.#device.releaseInterface(this.#interfaceNumber);
    } catch (error) {
      failures.push(
        Tag("TransportCloseFailed", {
          detail: `release USB interface: ${describeHostError(error)}`,
        }),
      );
    }
    try {
      await this.#device.close();
    } catch (error) {
      failures.push(
        Tag("TransportCloseFailed", {
          detail: `close USB device: ${describeHostError(error)}`,
        }),
      );
    }
    return failures;
  }
}

/// Fixed 8-byte host tag for the config lane. The data session derives its tag
/// from the registered interface id; the config session registers no interface
/// (it bypasses the Reticulum runtime), so it advertises a stable synthetic
/// tag. The device stores it as the peer tag and never routes Reticulum traffic
/// to it because no ConfigRequest the device sends is ever expected back.
const USB_AUTO_CONFIG_HOST_TAG = new Uint8Array([
  0x70, 0x72, 0x6e, 0x73, 0x68, 0x6f, 0x73, 0x74, // "prnshost"
]);

/// Per-request ceiling for a ConfigResponse / Snapshot round trip. The device
/// applies actions synchronously inside the config task, so this only fires on
/// a wedged radio or a dropped frame; resolving with a failure beats hanging
/// the webUI forever.
const USB_AUTO_CONFIG_REQUEST_TIMEOUT_MS = 5_000;

export type UsbAutoConfigConnectOutcome =
  | Tag<"Connected", BrowserUsbAutoConfigSession>
  | HostApiUnavailable<"WebUSB">
  | PermissionDenied<"usb-auto">
  | Cancelled<"usb-auto">
  | UnsupportedDevice<"usb-auto">
  | ConnectionFailed<"usb-auto">;

export type UsbAutoConfigActionOutcome =
  | Tag<"Result", UsbAutoConfigResult>
  | InterfaceSessionFailure;

export type UsbAutoConfigSnapshotOutcome =
  | Tag<"Snapshot", UsbAutoSnapshot>
  | InterfaceSessionFailure;

export type UsbAutoConfigConnectOptions = {
  readonly filters?: readonly UsbAutoDeviceFilter[];
};

/// Default WebUSB request filter for the config lane: the Prns VID/PID the
/// firmware enumerates as. Matches the data session's filter set.
function defaultUsbAutoConfigFilters(
  wasm: PrnsWasmModule,
): readonly BrowserUsbDeviceFilter[] {
  return [
    {
      vendorId: wasm.usbAutoWebUsbVendorId(),
      productId: wasm.usbAutoWebUsbProductId(),
    },
  ];
}

/// Headless config entry point for `/configure`. Unlike `UsbAutoInterface`,
/// this does NOT register a Reticulum interface or spin up a runtime lane — it
/// talks WebUSB + the wasm codec directly, so it works on a device with no
/// Reticulum data session (and side-by-side with one that does, since the
/// config-lane capability bit is a strict superset the device already speaks).
export class UsbAutoConfigInterface {
  readonly #wasm: PrnsWasmModule;

  constructor(wasm: PrnsWasmModule) {
    this.#wasm = wasm;
  }

  async connect(
    options: UsbAutoConfigConnectOptions = {},
  ): Promise<UsbAutoConfigConnectOutcome> {
    const available = requireWebUsb();
    if (available.tag !== "Available") {
      return available;
    }
    let transport: WebUsbAutoTransport | undefined;
    let stage: InterfaceConnectStage = "DeviceSelection";
    try {
      const requested = await usbStage("DeviceSelection", "request device", () =>
        available.data.requestDevice({
          filters: options.filters ?? defaultUsbAutoConfigFilters(this.#wasm),
        }),
      );
      if (requested.tag !== "Completed") {
        return requested;
      }
      stage = "TransportOpen";
      const opened = await WebUsbAutoTransport.open(requested.data);
      if (opened.tag !== "Opened") {
        return opened;
      }
      transport = opened.data;
      const session = new BrowserUsbAutoConfigSession(this.#wasm, transport);
      session.start();
      return Tag("Connected", session);
    } catch (error) {
      await transport?.close();
      return connectFailure("usb-auto", stage, error);
    }
  }
}

/// WebUSB session for the headless config lane. Decodes inbound frames, runs
/// the Hello/HelloAck handshake, and correlates outbound `ConfigRequest`s with
/// inbound `ConfigResponse`s (by request id) and `Snapshot` replies (uncorrelated,
/// since a snapshot carries no request id — at most one snapshot request is in
/// flight at a time).
class BrowserUsbAutoConfigSession {
  readonly #wasm: PrnsWasmModule;
  readonly #transport: WebUsbAutoTransport;
  readonly #decoder: UsbAutoDecoderBinding;
  #writeQueue: Promise<SessionWriteOutcome> = Promise.resolve(Tag("Written"));
  #closed = false;
  #confirmed = false;
  #status: InterfaceSessionStatus = Tag("Negotiating");
  #nextRequestId = 1;
  #nextToken = 1;
  #pendingResult:
    | {
        token: number;
        requestId: number;
        resolve: (outcome: UsbAutoConfigActionOutcome) => void;
      }
    | undefined;
  #pendingSnapshot:
    | {
        token: number;
        resolve: (outcome: UsbAutoConfigSnapshotOutcome) => void;
      }
    | undefined;

  constructor(wasm: PrnsWasmModule, transport: WebUsbAutoTransport) {
    this.#wasm = wasm;
    this.#transport = transport;
    this.#decoder = new wasm.UsbAutoDecoder();
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  start(): void {
    void this.#readLoop();
    void this.#probeLoop();
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#resolvePending(
      Tag("TransferFailed", {
        direction: "Outbound",
        detail: "session closed",
      }),
    );
    await this.#writeQueue;
    await this.#transport.close();
    this.#status = Tag("Closed");
  }

  /// Send `action` (bytes from a `usbAutoConfigAction*` builder) and await the
  /// matching `ConfigResponse`. The request id is allocated internally.
  async sendAction(
    action: Uint8Array,
  ): Promise<UsbAutoConfigActionOutcome> {
    if (this.#closed) {
      return Tag("TransferFailed", {
        direction: "Outbound",
        detail: "session closed",
      });
    }
    const requestId = this.#allocateRequestId();
    const written = await this.#writeFrame(
      this.#wasm.usbAutoConfigRequestFrame(requestId, action),
    );
    if (written.tag !== "Written") {
      return written;
    }
    return new Promise<UsbAutoConfigActionOutcome>((resolve) => {
      const token = this.#nextToken;
      this.#nextToken = (this.#nextToken + 1) | 0;
      this.#pendingResult = { token, requestId, resolve };
      void this.#armTimeout(token, (failure) => resolve(failure));
    });
  }

  /// Request a full device snapshot and decode its body into the webUI shape.
  async requestSnapshot(): Promise<UsbAutoConfigSnapshotOutcome> {
    if (this.#closed) {
      return Tag("TransferFailed", {
        direction: "Outbound",
        detail: "session closed",
      });
    }
    const requestId = this.#allocateRequestId();
    const written = await this.#writeFrame(
      this.#wasm.usbAutoConfigRequestFrame(
        requestId,
        this.#wasm.usbAutoConfigActionRequestSnapshot(),
      ),
    );
    if (written.tag !== "Written") {
      return written;
    }
    return new Promise<UsbAutoConfigSnapshotOutcome>((resolve) => {
      const token = this.#nextToken;
      this.#nextToken = (this.#nextToken + 1) | 0;
      this.#pendingSnapshot = { token, resolve };
      void this.#armTimeout(token, (failure) => resolve(failure));
    });
  }

  async #readLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const read = await this.#transport.read();
        if (read.tag !== "Read") {
          await this.#fail(read);
          return;
        }
        const chunk = read.data;
        if (!chunk) {
          break;
        }
        if (chunk.length === 0) {
          continue;
        }
        let messages: unknown[];
        try {
          messages = this.#decoder.feed(chunk);
        } catch (error) {
          await this.#fail(
            Tag("ProtocolViolation", {
              protocol: "UsbAuto",
              detail: describeHostError(error),
            }),
          );
          return;
        }
        for (const raw of messages) {
          let message: UsbAutoInboundMessage;
          try {
            message = parseUsbAutoMessage(raw);
          } catch (error) {
            await this.#fail(
              Tag("ProtocolViolation", {
                protocol: "UsbAuto",
                detail: describeHostError(error),
              }),
            );
            return;
          }
          const handled = await this.#handleInbound(message);
          if (handled.tag !== "Handled") {
            await this.#fail(handled);
            return;
          }
        }
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    } finally {
      if (!this.#closed) {
        await this.close();
      }
    }
  }

  async #probeLoop(): Promise<void> {
    try {
      while (!this.#closed && !this.#confirmed) {
        const written = await this.#writeFrame(this.#wasm.usbAutoHostHelloFrame());
        if (written.tag !== "Written") {
          await this.#fail(written);
          return;
        }
        await delay(USB_AUTO_PROBE_INTERVAL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #handleInbound(message: UsbAutoInboundMessage): Promise<SessionHandleOutcome> {
    return match_into<Promise<SessionHandleOutcome>>().from(message, {
      Hello: async () => {
        const written = await this.#writeFrame(
          this.#wasm.usbAutoHostHelloAckFrame(USB_AUTO_CONFIG_HOST_TAG),
        );
        if (written.tag !== "Written") {
          return written;
        }
        this.#confirmPeer();
        return Tag("Handled");
      },
      HelloAck: async () => {
        this.#confirmPeer();
        return Tag("Handled");
      },
      // The config session ignores Reticulum data; the data session owns it.
      Data: async () => Tag("Handled"),
      // The device is the config responder; it never sends ConfigRequest to us.
      ConfigRequest: async () => Tag("Handled"),
      ConfigResponse: async ({ requestId, result }) => {
        const pending = this.#pendingResult;
        if (pending && pending.requestId === requestId) {
          this.#pendingResult = undefined;
          pending.resolve(Tag("Result", result));
        }
        return Tag("Handled");
      },
      Snapshot: async ({ body }) => {
        const pending = this.#pendingSnapshot;
        if (pending) {
          this.#pendingSnapshot = undefined;
          try {
            pending.resolve(Tag("Snapshot", this.#wasm.usbAutoSnapshotDecode(body)));
          } catch (error) {
            pending.resolve(
              Tag("ProtocolViolation", {
                protocol: "UsbAuto",
                detail: describeHostError(error),
              }),
            );
          }
        }
        return Tag("Handled");
      },
    });
  }

  #confirmPeer(): void {
    this.#confirmed = true;
    this.#status = Tag("Active");
  }

  #allocateRequestId(): number {
    const id = this.#nextRequestId;
    this.#nextRequestId = (this.#nextRequestId + 1) & 0xff;
    if (this.#nextRequestId === 0) {
      this.#nextRequestId = 1;
    }
    return id;
  }

  /// Resolve any in-flight request with `failure` and clear both pending slots.
  /// Used on close and on session failure so callers never hang.
  #resolvePending(failure: InterfaceSessionFailure): void {
    const result = this.#pendingResult;
    if (result) {
      this.#pendingResult = undefined;
      result.resolve(failure);
    }
    const snapshot = this.#pendingSnapshot;
    if (snapshot) {
      this.#pendingSnapshot = undefined;
      snapshot.resolve(failure);
    }
  }

  /// Fail a pending request after the round-trip ceiling. `token` identifies
  /// which in-flight request this timeout owns; if the dispatcher already
  /// resolved it (slot cleared or reused for a later request), the timeout is a
  /// no-op.
  #armTimeout(
    token: number,
    fail: (failure: InterfaceSessionFailure) => void,
  ): void {
    void delay(USB_AUTO_CONFIG_REQUEST_TIMEOUT_MS).then(() => {
      if (this.#pendingResult?.token === token) {
        this.#pendingResult = undefined;
        fail(
          Tag("TransferFailed", {
            direction: "Inbound",
            detail: "config request timed out",
          }),
        );
        return;
      }
      if (this.#pendingSnapshot?.token === token) {
        this.#pendingSnapshot = undefined;
        fail(
          Tag("TransferFailed", {
            direction: "Inbound",
            detail: "snapshot request timed out",
          }),
        );
      }
    });
  }

  async #fail(sessionFailure: InterfaceSessionFailure): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#status = Tag("Failed", sessionFailure);
    this.#closed = true;
    this.#resolvePending(sessionFailure);
    await this.#writeQueue;
    await this.#transport.close();
  }

  async #writeFrame(frame: Uint8Array): Promise<SessionWriteOutcome> {
    if (this.#closed) {
      return Tag("Written");
    }
    const write = this.#writeQueue
      .then(async (previous): Promise<SessionWriteOutcome> => {
        if (previous.tag !== "Written" || this.#closed) {
          return previous;
        }
        return this.#transport.write(frame);
      })
      .catch((error: unknown) => unexpectedSessionFailure(error));
    this.#writeQueue = write;
    return write;
  }
}

export class WebSocketInterface {
  readonly name = "websocket" as const;
  readonly #host: RuntimeHost;
  readonly #activeTags = new Set<string>();

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(
    url: string | URL,
    options: WebSocketConnectOptions = {},
  ): Promise<WebSocketConnectOutcome> {
    const ready = this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    const canonical = canonicalWebSocketUrl(url);
    if (canonical.tag !== "Canonical") {
      return canonical;
    }
    const target = canonical.data;
    const protocols = normalizedWebSocketProtocols(options.protocols);
    let tag: ChannelTag;
    try {
      tag = options.channelTag ?? browserWebSocketChannelTag(target, protocols);
    } catch (error) {
      return connectFailure("websocket", "RuntimeRegistration", error);
    }
    const tagKey = byteKey(tag);
    if (this.#activeTags.has(tagKey)) {
      return Tag("AlreadyActive", { interface: "websocket", target });
    }
    this.#activeTags.add(tagKey);

    let socket: WebSocket | undefined;
    let interfaceId: InterfaceId | undefined;
    let stage: InterfaceConnectStage = "TransportOpen";
    try {
      const opened = await openBrowserWebSocket(target, protocols);
      if (opened.tag !== "Opened") {
        this.#activeTags.delete(tagKey);
        return opened;
      }
      socket = opened.data;
      stage = "RuntimeRegistration";
      const registered = this.#host.registerInterface({
        interfaceName: "websocket",
        kind: "websocket-client",
        channelTag: tag,
        bitrateBps: options.bitrateBps ?? this.#host.websocketBitrateBps(),
        hardwareMtu: options.hardwareMtu ?? this.#host.websocketHardwareMtu(),
        ...runtimeInterfaceRouting(options.routing),
      });
      if (registered.tag !== "Registered") {
        closeBrowserWebSocket(socket);
        this.#activeTags.delete(tagKey);
        return registered;
      }
      interfaceId = registered.data;
      stage = "Handshake";
      const session = new BrowserWebSocketSession(
        this.#host,
        socket,
        interfaceId,
        target,
        this.#host.websocketFrameCap(),
        () => this.#activeTags.delete(tagKey),
      );
      session.start();
      return Tag("Connected", session);
    } catch (error) {
      if (interfaceId) {
        this.#host.deactivateInterface(interfaceId);
      }
      closeBrowserWebSocket(socket);
      this.#activeTags.delete(tagKey);
      return connectFailure("websocket", stage, error);
    }
  }
}

class BrowserWebSocketSession implements WebSocketSession {
  readonly name = "websocket" as const;
  readonly interfaceId: InterfaceId;
  readonly url: string;

  readonly #host: RuntimeHost;
  readonly #socket: WebSocket;
  readonly #frameCap: number;
  readonly #bufferLimit: number;
  readonly #release: () => void;
  #readQueue: Promise<void> = Promise.resolve();
  #writeQueue: Promise<SessionWriteOutcome> = Promise.resolve(Tag("Written"));
  #closed = false;
  #released = false;
  #status: InterfaceSessionStatus = Tag("Active");

  constructor(
    host: RuntimeHost,
    socket: WebSocket,
    interfaceId: InterfaceId,
    url: string,
    frameCap: number,
    release: () => void,
  ) {
    this.#host = host;
    this.#socket = socket;
    this.interfaceId = interfaceId;
    this.url = url;
    this.#frameCap = frameCap;
    this.#bufferLimit = Math.max(WEBSOCKET_MIN_BUFFER_LIMIT, frameCap * 2);
    this.#release = release;
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  start(): void {
    this.#socket.addEventListener("message", (event) => {
      this.#enqueueMessage(event);
    });
    this.#socket.addEventListener("close", () => {
      this.#handleClose();
    });
    this.#socket.addEventListener("error", () => {
      void this.#fail(
        Tag("Disconnected", {
          detail: `WebSocket connection failed for ${this.url}`,
        }),
      );
    });
    void this.#outboundLoop();
  }

  async close(): Promise<InterfaceCloseOutcome> {
    if (this.#closed) {
      return closedSessionOutcome(this.#status);
    }
    this.#closed = true;
    const causes: InterfaceCleanupFailure[] = [];
    const detached = this.#host.deactivateInterface(this.interfaceId);
    if (detached.tag !== "Detached") {
      causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
    }
    this.#releaseOnce();
    const socketFailure = closeBrowserWebSocket(this.#socket);
    if (socketFailure) {
      causes.push(socketFailure);
    }
    const pendingWrite = await this.#writeQueue;
    if (pendingWrite.tag !== "Written") {
      causes.push(
        Tag("TransportCloseFailed", {
          detail: describeInterfaceSessionFailure(pendingWrite),
        }),
      );
    }
    if (hasCleanupFailures(causes)) {
      const failed = closeFailed(causes);
      this.#status = Tag("Failed", failed);
      return failed;
    }
    this.#status = Tag("Closed");
    return Tag("Closed");
  }

  #enqueueMessage(event: MessageEvent): void {
    this.#readQueue = this.#readQueue
      .then(async () => {
        const handled = await this.#handleMessage(event);
        if (handled.tag !== "Handled" && !this.#closed) {
          await this.#fail(handled);
        }
      })
      .catch(async (error: unknown) => {
        if (!this.#closed) {
          await this.#fail(unexpectedSessionFailure(error));
        }
      });
  }

  async #handleMessage(event: MessageEvent): Promise<SessionHandleOutcome> {
    const decoded = await websocketMessageBytes(event.data, this.#frameCap);
    if (decoded.tag !== "Decoded") {
      return decoded;
    }
    if (decoded.data.length > 0 && !this.#closed) {
      const ingested = this.#host.ingest(
        this.interfaceId,
        packetFrame(decoded.data),
      );
      return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
    }
    return Tag("Handled");
  }

  #handleClose(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    const detached = this.#host.deactivateInterface(this.interfaceId);
    this.#status =
      detached.tag === "Detached" ? Tag("Closed") : Tag("Failed", detached);
    this.#releaseOnce();
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const outbound = this.#host.takeOutboundFor(this.interfaceId);
        if (outbound.tag !== "Outbound") {
          await this.#fail(outbound);
          return;
        }
        for (const frame of outbound.data) {
          const written = await this.#writeFrame(frame.bytes);
          if (written.tag !== "Written") {
            await this.#fail(written);
            return;
          }
        }
        await delay(WEBSOCKET_OUTBOUND_POLL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #fail(sessionFailure: InterfaceSessionFailure): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#status = Tag("Failed", sessionFailure);
    this.#closed = true;
    this.#host.deactivateInterface(this.interfaceId);
    this.#releaseOnce();
    await this.#writeQueue;
    closeBrowserWebSocket(this.#socket);
  }

  async #writeFrame(frame: Uint8Array): Promise<SessionWriteOutcome> {
    if (this.#closed || frame.length === 0) {
      return Tag("Written");
    }
    if (frame.length > this.#frameCap) {
      return Tag("FrameTooLarge", {
        length: frame.length,
        maximum: this.#frameCap,
      });
    }
    const write = this.#writeQueue
      .then(async (previous): Promise<SessionWriteOutcome> => {
        if (previous.tag !== "Written" || this.#closed) {
          return previous;
        }
        while (!this.#closed && this.#socket.bufferedAmount > this.#bufferLimit) {
          await delay(WEBSOCKET_BUFFER_POLL_MS);
        }
        if (this.#closed) {
          return Tag("Written");
        }
        if (this.#socket.readyState !== WEBSOCKET_OPEN) {
          return Tag("Disconnected", {
            detail: `WebSocket is not open for ${this.url}`,
          });
        }
        try {
          this.#socket.send(frame);
          return Tag("Written");
        } catch (error) {
          return Tag("Disconnected", { detail: describeHostError(error) });
        }
      })
      .catch((error: unknown) => unexpectedSessionFailure(error));
    this.#writeQueue = write;
    return write;
  }

  #releaseOnce(): void {
    if (!this.#released) {
      this.#released = true;
      this.#release();
    }
  }
}

export class RNodeInterface {
  readonly name = "rnode" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(): Promise<RNodeConnectOutcome> {
    const ready = this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    return Tag("UnsupportedInterface", {
      interface: "rnode",
      host: "Browser",
    });
  }
}

export class BluetoothInterface {
  readonly name = "bluetooth" as const;
  readonly #host: RuntimeHost;

  constructor(host: RuntimeHost) {
    this.#host = host;
  }

  async connect(): Promise<BluetoothConnectOutcome> {
    const identity = this.#host.bluetoothIdentityReadiness();
    if (identity.tag !== "Ready") {
      return identity;
    }
    const ready = this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    const available = requireWebBluetooth();
    if (available.tag !== "Available") {
      return available;
    }
    let server: BrowserBluetoothRemoteGattServer | undefined;
    let session: BrowserBluetoothSession | undefined;
    let stage: InterfaceConnectStage = "DeviceSelection";
    try {
      const serviceUuid = this.#host.bluetoothServiceUuid();
      const requested = await bluetoothStage(
        "DeviceSelection",
        () =>
          available.data.requestDevice({
            filters: [{ services: [serviceUuid] }],
            optionalServices: [serviceUuid],
          }),
      );
      if (requested.tag !== "Completed") {
        return requested;
      }
      const gatt = requested.data.gatt;
      if (!gatt) {
        return Tag("UnsupportedDevice", {
          interface: "bluetooth",
          capability: "GATT server",
        });
      }
      stage = "TransportOpen";
      const connected = await bluetoothStage(
        "TransportOpen",
        () => gatt.connect(),
      );
      if (connected.tag !== "Completed") {
        return connected;
      }
      const connectedServer = connected.data;
      server = connectedServer;
      stage = "ServiceDiscovery";
      const discovered = await bluetoothStage(
        "ServiceDiscovery",
        () => connectedServer.getPrimaryService(serviceUuid),
      );
      if (discovered.tag !== "Completed") {
        disconnectBluetoothServer(connectedServer);
        return discovered;
      }
      const control = await bluetoothStage(
        "ServiceDiscovery",
        () =>
          discovered.data.getCharacteristic(this.#host.bluetoothControlUuid()),
      );
      if (control.tag !== "Completed") {
        disconnectBluetoothServer(connectedServer);
        return control;
      }
      const data = await optionalBluetoothCharacteristic(
        discovered.data,
        this.#host.bluetoothDataUuid(),
      );
      stage = "Handshake";
      session = new BrowserBluetoothSession(
        this.#host,
        connectedServer,
        control.data,
        data ?? control.data,
      );
      const started = await session.start();
      if (started.tag !== "Started") {
        await session.close();
        return started;
      }
      return Tag("Connected", session);
    } catch (error) {
      if (session) {
        await session.close();
      } else if (server) {
        disconnectBluetoothServer(server);
      }
      return connectFailure("bluetooth", stage, error);
    }
  }
}

class BrowserBluetoothSession implements BluetoothSession {
  readonly name = "bluetooth" as const;
  readonly #host: RuntimeHost;
  readonly #server: BrowserBluetoothRemoteGattServer;
  readonly #control: BrowserBluetoothRemoteGattCharacteristic;
  readonly #data: BrowserBluetoothRemoteGattCharacteristic;
  readonly #reassembler: BluetoothReassemblerBinding;
  #interfaceId?: InterfaceId;
  #writeQueue: Promise<SessionWriteOutcome> = Promise.resolve(Tag("Written"));
  #closed = false;
  #confirmed = false;
  #status: InterfaceSessionStatus = Tag("Negotiating");
  #connectFailure?: BluetoothConnectFailure;

  constructor(
    host: RuntimeHost,
    server: BrowserBluetoothRemoteGattServer,
    control: BrowserBluetoothRemoteGattCharacteristic,
    data: BrowserBluetoothRemoteGattCharacteristic,
  ) {
    this.#host = host;
    this.#server = server;
    this.#control = control;
    this.#data = data;
    this.#reassembler = host.createBluetoothReassembler();
  }

  get interfaceId(): InterfaceId {
    if (!this.#interfaceId) {
      throw new PrnsValidationError(
        "invalid-component",
        "Bluetooth peer interface is not registered yet",
      );
    }
    return this.#interfaceId;
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  async start(): Promise<BluetoothStartOutcome> {
    const controlStarted = await bluetoothStage(
      "Handshake",
      () => this.#control.startNotifications(),
    );
    if (controlStarted.tag !== "Completed") {
      return controlStarted;
    }
    this.#control.addEventListener("characteristicvaluechanged", (event) => {
      try {
        const handled = this.#handleControlEvent(
          event as BrowserBluetoothCharacteristicEvent,
        );
        if (handled.tag !== "Handled") {
          this.#handleEventFailure(handled);
        }
      } catch (error) {
        this.#handleEventFailure(unexpectedSessionFailure(error));
      }
    });
    if (this.#data !== this.#control) {
      const dataStarted = await bluetoothStage(
        "Handshake",
        () => this.#data.startNotifications(),
      );
      if (dataStarted.tag !== "Completed") {
        return dataStarted;
      }
      this.#data.addEventListener("characteristicvaluechanged", (event) => {
        try {
          const handled = this.#handleDataEvent(
            event as BrowserBluetoothCharacteristicEvent,
          );
          if (handled.tag !== "Handled") {
            this.#handleEventFailure(handled);
          }
        } catch (error) {
          this.#handleEventFailure(unexpectedSessionFailure(error));
        }
      });
    }
    const written = await this.#writeControl(this.#host.bluetoothDialerHello());
    if (written.tag !== "Written") {
      return sessionFailureToConnectFailure("bluetooth", "Handshake", written);
    }
    const confirmed = await this.#waitForPeer();
    if (confirmed.tag !== "Confirmed") {
      return confirmed;
    }
    void this.#outboundLoop();
    return Tag("Started");
  }

  async close(): Promise<InterfaceCloseOutcome> {
    if (this.#closed) {
      return closedSessionOutcome(this.#status);
    }
    this.#closed = true;
    const causes: InterfaceCleanupFailure[] = [];
    if (this.#interfaceId) {
      const detached = this.#host.deactivateInterface(this.#interfaceId);
      if (detached.tag !== "Detached") {
        causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
      }
    }
    const pendingWrite = await this.#writeQueue;
    if (pendingWrite.tag !== "Written") {
      causes.push(
        Tag("TransportCloseFailed", {
          detail: describeInterfaceSessionFailure(pendingWrite),
        }),
      );
    }
    const disconnected = disconnectBluetoothServer(this.#server);
    if (disconnected) {
      causes.push(disconnected);
    }
    if (hasCleanupFailures(causes)) {
      const failed = closeFailed(causes);
      this.#status = Tag("Failed", failed);
      return failed;
    }
    this.#status = Tag("Closed");
    return Tag("Closed");
  }

  async #waitForPeer(): Promise<Tag<"Confirmed"> | BluetoothConnectFailure> {
    const started = Date.now();
    while (!this.#confirmed && !this.#closed && !this.#connectFailure) {
      if (Date.now() - started > BLUETOOTH_HANDSHAKE_TIMEOUT_MS) {
        const timedOut: ConnectTimedOut<"bluetooth"> = Tag("TimedOut", {
          interface: "bluetooth",
          stage: "Handshake",
          timeoutMs: BLUETOOTH_HANDSHAKE_TIMEOUT_MS,
        });
        this.#abortConnect(timedOut);
        return timedOut;
      }
      await delay(25);
    }
    if (this.#connectFailure) {
      return this.#connectFailure;
    }
    if (!this.#confirmed) {
      return Tag("ConnectionFailed", {
        interface: "bluetooth",
        stage: "Handshake",
        detail: "Bluetooth link closed before peer confirmation",
      });
    }
    return Tag("Confirmed");
  }

  #handleControlEvent(
    event: BrowserBluetoothCharacteristicEvent,
  ): BluetoothHandleOutcome {
    const decoded = characteristicBytes(event);
    if (decoded.tag !== "Decoded") {
      return decoded;
    }
    const bytes = decoded.data;
    let control: BluetoothControl;
    try {
      control = parseBluetoothControl(this.#host.bluetoothDecodeControl(bytes));
    } catch (error) {
      return Tag("ProtocolViolation", {
        protocol: "Bluetooth",
        detail: describeHostError(error),
      });
    }
    return match_into<BluetoothHandleOutcome>().from(control, {
      Hello: () =>
        this.#data === this.#control
          ? this.#handleDataBytes(bytes)
          : Tag("Handled"),
      Welcome: (identity) => {
        if (this.#confirmed) {
          return Tag("Handled");
        }
        let registration: HostedInterfaceRegistration<"bluetooth">;
        try {
          registration = {
            interfaceName: "bluetooth",
            supervisorKind: "bluetooth-auto",
            kind: "bluetooth-peer",
            channelTag: channelTag(identity),
            bitrateBps: this.#host.bluetoothBitrateBps(),
            hardwareMtu: this.#host.bluetoothHardwareMtu(),
          };
        } catch (error) {
          return Tag("ProtocolViolation", {
            protocol: "Bluetooth",
            detail: describeHostError(error),
          });
        }
        const registered = this.#host.registerInterface(registration);
        if (registered.tag !== "Registered") {
          return registered;
        }
        this.#interfaceId = registered.data;
        this.#confirmed = true;
        this.#status = Tag("Active");
        return Tag("Handled");
      },
      Close: () => {
        void this.close();
        return Tag("Handled");
      },
    });
  }

  #handleDataEvent(
    event: BrowserBluetoothCharacteristicEvent,
  ): SessionHandleOutcome {
    const decoded = characteristicBytes(event);
    return decoded.tag === "Decoded"
      ? this.#handleDataBytes(decoded.data)
      : decoded;
  }

  #handleDataBytes(bytes: Uint8Array): SessionHandleOutcome {
    if (!this.#confirmed || !this.#interfaceId) {
      return Tag("Handled");
    }
    let frame: Uint8Array | undefined;
    try {
      frame = this.#reassembler.absorb(bytes);
    } catch (error) {
      return Tag("ProtocolViolation", {
        protocol: "Bluetooth",
        detail: describeHostError(error),
      });
    }
    if (frame && frame.length > 0) {
      const ingested = this.#host.ingest(this.#interfaceId, packetFrame(frame));
      return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
    }
    return Tag("Handled");
  }

  #handleEventFailure(
    failure: InterfaceSessionFailure | AlreadyActive<"bluetooth">,
  ): void {
    if (!this.#confirmed) {
      this.#abortConnect(
        failure.tag === "AlreadyActive"
          ? failure
          : sessionFailureToConnectFailure("bluetooth", "Handshake", failure),
      );
      return;
    }
    const sessionFailure =
      failure.tag === "AlreadyActive"
        ? unexpectedSessionFailure(
            `Bluetooth peer became active more than once for ${failure.data.target}`,
          )
        : failure;
    void this.#fail(sessionFailure);
  }

  #abortConnect(failure: BluetoothConnectFailure): void {
    if (this.#closed) {
      return;
    }
    this.#connectFailure = failure;
    this.#status = Tag(
      "Failed",
      failure.tag === "RuntimeRejected"
        ? failure
        : unexpectedSessionFailure(describeBluetoothConnectFailure(failure)),
    );
    this.#closed = true;
    if (this.#interfaceId) {
      this.#host.deactivateInterface(this.#interfaceId);
    }
    disconnectBluetoothServer(this.#server);
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const interfaceId = this.#interfaceId;
        if (this.#confirmed && interfaceId) {
          const outbound = this.#host.takeOutboundFor(interfaceId);
          if (outbound.tag !== "Outbound") {
            await this.#fail(outbound);
            return;
          }
          for (const frame of outbound.data) {
            for (const fragment of this.#host.bluetoothDataFragments(frame.bytes)) {
              const written = await this.#writeData(fragment);
              if (written.tag !== "Written") {
                await this.#fail(written);
                return;
              }
            }
          }
        }
        await delay(BLUETOOTH_OUTBOUND_POLL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #fail(sessionFailure: InterfaceSessionFailure): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#status = Tag("Failed", sessionFailure);
    this.#closed = true;
    if (this.#interfaceId) {
      this.#host.deactivateInterface(this.#interfaceId);
    }
    await this.#writeQueue;
    disconnectBluetoothServer(this.#server);
  }

  async #writeControl(bytes: Uint8Array): Promise<SessionWriteOutcome> {
    return this.#write(this.#control, bytes);
  }

  async #writeData(bytes: Uint8Array): Promise<SessionWriteOutcome> {
    return this.#write(this.#data, bytes);
  }

  async #write(
    characteristic: BrowserBluetoothRemoteGattCharacteristic,
    bytes: Uint8Array,
  ): Promise<SessionWriteOutcome> {
    if (this.#closed || bytes.length === 0) {
      return Tag("Written");
    }
    const write = this.#writeQueue
      .then(async (previous): Promise<SessionWriteOutcome> => {
        if (previous.tag !== "Written" || this.#closed) {
          return previous;
        }
        return writeBluetoothValue(characteristic, bytes);
      })
      .catch((error: unknown) => unexpectedSessionFailure(error));
    this.#writeQueue = write;
    return write;
  }
}

export class Prns {
  readonly interfaces: PrnsInterfaces;
  #runtime: PrnsRuntimeBinding;
  #host: RuntimeHost;
  #entropy: EntropySource;
  #now: () => InstantMillis;
  #startedAtMillis: number;
  #limits: HostLimits;
  #resourceCompressionModuleUrl: string;
  #events: BoundedAsyncLane<PrnsApplicationEvent>;
  #diagnostics: BoundedAsyncLane<PrnsDiagnosticEvent>;
  #pendingCommands = new Map<
    bigint,
    {
      pending: PendingCommand;
      settle: (settlement: CommandSettlement) => void;
    }
  >();
  #responseParts = new Map<bigint, Uint8Array[]>();
  #attachedInterfaces = new Map<string, InterfaceSession>();
  #lifecycle: HostLifecycleState = Tag("Running");
  #stopCompleted = false;
  #stopPromise: Promise<StopOutcome> | undefined;
  #persistenceStore: BrowserPersistenceStore | undefined;
  #persistenceRestored: boolean;
  #lastPersistenceFlushCause: "Shutdown" | undefined;
  #persistenceFailureDetail: string | undefined;

  private constructor(
    wasm: PrnsWasmModule,
    runtime: PrnsRuntimeBinding,
    entropy: EntropySource,
    now: () => InstantMillis,
    bleIdentityAvailability: BleIdentityAvailability,
    limits: HostLimits,
    resourceCompressionModuleUrl: URL,
    persistenceStore: BrowserPersistenceStore | undefined,
    persistenceRestored: boolean,
    restorationReport: BrowserPersistenceRestoreReport | undefined,
  ) {
    this.#runtime = runtime;
    this.#entropy = entropy;
    this.#now = now;
    this.#startedAtMillis = now();
    this.#limits = limits;
    this.#resourceCompressionModuleUrl =
      resourceCompressionModuleUrl.href;
    this.#persistenceStore = persistenceStore;
    this.#persistenceRestored = persistenceRestored;
    this.#events = new BoundedAsyncLane<PrnsApplicationEvent>({
      name: "ApplicationEvents",
      maximumValues: limits.applicationEvents,
      maximumBytes: limits.retainedEventBytes,
      measure: retainedBrowserEventBytes,
      onRejected: (rejectedEventBytes) =>
        this.#failBackpressure(rejectedEventBytes),
      onBeforeNext: () => this.#pumpEvents(),
    });
    this.#diagnostics = new BoundedAsyncLane<PrnsDiagnosticEvent>({
      name: "Diagnostics",
      maximumValues: limits.diagnostics,
      maximumBytes: Number.MAX_SAFE_INTEGER,
      measure: () => 0,
      gap: (count) => Tag("DiagnosticsDropped", { count }),
      onBeforeNext: () => this.#pumpEvents(),
    });
    this.#host = new RuntimeHost(
      wasm,
      runtime,
      entropy,
      now,
      bleIdentityAvailability,
      () => this.#pumpEvents(),
    );
    this.interfaces = new PrnsInterfaces(this.#host);
    if (restorationReport !== undefined) {
      this.#diagnostics.push(Tag("PersistenceRestored", restorationReport));
    }
  }

  static async create(options: PrnsOptions): Promise<PrnsCreateOutcome> {
    const loaded = options.wasm
      ? Tag("Loaded", options.wasm)
      : await loadBundledWasm();
    if (loaded.tag !== "Loaded") {
      return loaded;
    }
    const wasm = loaded.data;
    let actualAbi: number;
    let actualSchemaVersion: number;
    let actualPersistenceVersion: number;
    let actualProductVersion: string;
    try {
      actualAbi = wasm.hostContractAbi();
      actualSchemaVersion = wasm.hostSchemaVersion();
      actualPersistenceVersion = wasm.browserPersistenceVersion();
      actualProductVersion = wasm.productVersion();
    } catch (error) {
      return runtimeRejected("initialize", error);
    }
    if (
      actualAbi !== HOST_CONTRACT_ABI ||
      actualSchemaVersion !== HOST_SCHEMA_VERSION ||
      actualProductVersion !== PRODUCT_VERSION
    ) {
      return Tag("ContractMismatch", {
        requiredAbi: HOST_CONTRACT_ABI,
        actualAbi,
        requiredSchemaVersion: HOST_SCHEMA_VERSION,
        actualSchemaVersion,
        requiredProductVersion: PRODUCT_VERSION,
        actualProductVersion,
      });
    }
    if (actualPersistenceVersion !== BROWSER_PERSISTENCE_VERSION) {
      return runtimeRejected(
        "initialize",
        `browser persistence version ${actualPersistenceVersion} does not match ${BROWSER_PERSISTENCE_VERSION}`,
      );
    }
    let identityLength: number;
    try {
      identityLength = positiveInteger(
        wasm.identitySecretKeyLength(),
        "identity secret key length",
      );
    } catch (error) {
      return runtimeRejected("initialize", error);
    }
    const store = options.identityStore;
    let identity: IdentitySecretKey | undefined;
    if (store) {
      let loaded: IdentityLoadOutcome;
      try {
        loaded = await store.load(identityLength);
      } catch (error) {
        return Tag("IdentityStoreFailed", {
          operation: "Load",
          detail: describeHostError(error),
        });
      }
      if (loaded.tag === "Loaded") {
        try {
          identity = identitySecretKey(loaded.data, identityLength);
        } catch (error) {
          return Tag("StoredIdentityInvalid", {
            detail: describeHostError(error),
          });
        }
      } else if (loaded.tag !== "Missing") {
        return loaded;
      }
    }
    if (!identity) {
      const generated = webCryptoIdentity(identityLength);
      if (generated.tag !== "Generated") {
        return generated;
      }
      identity = generated.data;
      if (store) {
        let saved: IdentitySaveOutcome;
        try {
          saved = await store.save(identity);
        } catch (error) {
          return Tag("IdentityStoreFailed", {
            operation: "Save",
            detail: describeHostError(error),
          });
        }
        if (saved.tag !== "Saved") {
          return saved;
        }
      }
    }
    const bleIdentityAvailability = await loadOrCreateBleIdentity(
      options.bleIdentityStore ?? new BrowserLocalStorageBleIdentityStore(),
    );
    const bleIdentity =
      bleIdentityAvailability.tag === "Available"
        ? bleIdentityAvailability.data
        : undefined;
    const persistenceStore = options.persistenceStore;
    let persistedState: BrowserPersistedState | undefined;
    if (persistenceStore !== undefined) {
      let loaded: PersistenceLoadOutcome;
      try {
        loaded = await persistenceStore.load();
      } catch (error) {
        return Tag("PersistenceStoreFailed", {
          operation: "Load",
          detail: describeHostError(error),
        });
      }
      if (loaded.tag === "Loaded") {
        try {
          persistedState = parseBrowserPersistedState(loaded.data);
        } catch (error) {
          return Tag("StoredPersistenceInvalid", {
            detail: describeHostError(error),
          });
        }
      } else if (loaded.tag !== "Missing") {
        return loaded;
      }
    }
    let limits: HostLimits;
    let now: () => InstantMillis;
    let runtime: PrnsRuntimeBinding;
    try {
      limits = browserLimits(options.limits ?? balancedLimits());
      now = options.now ?? nowMillis;
      runtime = new wasm.PrnsRuntime(identity, bleIdentity);
    } catch (error) {
      return runtimeRejected("initialize", error);
    }
    let restorationReport: BrowserPersistenceRestoreReport | undefined;
    if (persistedState !== undefined) {
      try {
        restorationReport = parsePersistenceRestoreReport(
          runtime.restorePersistedState({
            ...persistedState,
            nowMs: nowMillis(Math.max(now(), persistedState.takenAtMillis)),
          }),
        );
      } catch (error) {
        return Tag("StoredPersistenceInvalid", {
          detail: describeHostError(error),
        });
      }
    }
    try {
      return Tag(
        "Ready",
        new Prns(
          wasm,
          runtime,
          options.entropy ?? webCryptoEntropy,
          now,
          bleIdentityAvailability,
          limits,
          options.resourceCompressionModuleUrl ??
            bundledWasmModuleUrl(),
          persistenceStore,
          persistedState !== undefined,
          restorationReport,
        ),
      );
    } catch (error) {
      return runtimeRejected("initialize", error);
    }
  }

  registerSingleDestination(
    options: RegisterSingleDestinationOptions,
  ): DestinationRegistrationOutcome {
    try {
      return Tag(
        "Registered",
        destinationHash(this.#runtime.registerSingleDestination(options)),
      );
    } catch (error) {
      return runtimeRejected("register-destination", error);
    }
  }

  registerNodePage(appData: Uint8Array): DestinationRegistrationOutcome {
    try {
      return Tag(
        "Registered",
        destinationHash(this.#runtime.registerNodePage({ appData })),
      );
    } catch (error) {
      return runtimeRejected("register-node-page", error);
    }
  }

  execute<Command extends HostCommand>(
    command: Command,
  ): Promise<CommandSettlementFor<Command>> {
    return this.#execute(command) as Promise<CommandSettlementFor<Command>>;
  }

  #execute(command: HostCommand): Promise<CommandSettlement> {
    if (this.#lifecycle.tag !== "Running") {
      return Promise.resolve(commandFailed(Tag("NodeStopped")));
    }
    return match_into<Promise<CommandSettlement>>().from(command, {
      Announce: ({ destination, interface: interfaceId }) =>
        this.#issueCommand("announce", command, (entropy) =>
          this.#runtime.announce({
            destination,
            ...(interfaceId === undefined ? {} : { interfaceId }),
            nowMs: this.#now(),
            entropy,
          }),
        ),
      SendSinglePacket: ({ destination, payload }) =>
        this.#issueCommand("send-single-packet", command, (entropy) =>
          this.#runtime.sendSinglePacket({
            destination,
            payload,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      CloseLink: ({ linkId: value }) =>
        this.#issueCommand("close-link", command, (entropy) =>
          this.#runtime.closeLink({
            linkId: value,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      AttachTcpServer: async () =>
        commandFailed(Tag("UnsupportedByBackend")),
      AttachTcpClient: async () =>
        commandFailed(Tag("UnsupportedByBackend")),
      AttachUdp: async () =>
        commandFailed(Tag("UnsupportedByBackend")),
      AttachInterface: ({ config, routing }) => this.#attachInterface(config, routing),
      DetachInterface: ({ interface: interfaceId }) =>
        this.#detachInterface(interfaceId),
      EstablishLink: ({ destination }) =>
        this.#issueCommand("establish-link", command, (entropy) =>
          this.#runtime.establishLink({
            destination,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      RequestPath: ({ destination }) =>
        this.#issueCommand("request-path", command, (entropy) =>
          this.#runtime.requestPath({
            destination,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      Identify: ({ linkId: value, identity }) =>
        this.#issueCommand("identify", command, (entropy) =>
          this.#runtime.identify({
            linkId: value,
            identity,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      SendLinkPacket: ({ linkId: value, payload }) =>
        this.#issueCommand("send-link-packet", command, (entropy) =>
          this.#runtime.sendLinkPacket({
            linkId: value,
            payload,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      Request: ({
        linkId: value,
        pathHash,
        payload,
        timeout,
        maximumResponseBytes,
      }) =>
        this.#issueCommand("request", command, (entropy) =>
          this.#runtime.request({
            linkId: value,
            pathHash,
            payload,
            nowMs: this.#now(),
            entropy,
            ...runtimeResponseTimeout(timeout),
            ...(maximumResponseBytes === undefined
              ? {}
              : {
                  maximumResponseBytes: nonNegativeInteger(
                    maximumResponseBytes,
                    "maximumResponseBytes",
                  ),
                }),
          }),
        ),
      Respond: ({
        linkId: value,
        requestId: responseRequestId,
        requestRttMillis,
        payload,
      }) =>
        this.#issueCommand("respond", command, (entropy) =>
          this.#runtime.respond({
            linkId: value,
            requestId: responseRequestId,
            requestRttMillis,
            payload,
            nowMs: this.#now(),
            entropy,
          }),
        ),
      SendResource: ({
        linkId: value,
        payload,
        packedMetadata,
        compression,
      }) =>
        this.#sendResourceSource(
          value,
          byteResourceSource(payload),
          compression,
          packedMetadata,
        ),
      SetLinkResourceStrategy: ({ linkId: value, strategy }) =>
        this.#issueCommand(
          "set-link-resource-strategy",
          command,
          (entropy) =>
            this.#runtime.setLinkResourceStrategy({
              linkId: value,
              nowMs: this.#now(),
              entropy,
              ...runtimeResourceStrategy(strategy),
            }),
        ),
      SetDestinationResourceStrategy: async ({
        destination,
        strategy,
      }) => {
        try {
          const configured =
            this.#runtime.setDestinationResourceStrategy({
              destination,
              ...runtimeResourceStrategy(strategy),
            });
          return configured
            ? Tag("Succeeded", Tag("ResourceStrategySet"))
            : commandFailed(Tag("UnknownDestination"));
        } catch (error) {
          return commandFailed(
            browserCommandFailure(
              "set-destination-resource-strategy",
              error,
            ),
          );
        }
      },
      SendChannelMessage: ({
        linkId: value,
        messageType,
        payload,
      }) => {
        if (
          !Number.isSafeInteger(messageType) ||
          messageType < 0 ||
          messageType > 0xefff
        ) {
          return Promise.resolve(
            commandFailed(Tag("InvalidChannelMessageType")),
          );
        }
        return this.#issueCommand(
          "send-channel-message",
          command,
          (entropy) =>
            this.#runtime.sendChannelMessage({
              linkId: value,
              messageType,
              payload,
              nowMs: this.#now(),
              entropy,
            }),
        );
      },
      AllowRequester: ({ destination, pathHash, identity }) =>
        this.#issueCommand("allow-requester", command, (entropy) =>
          this.#runtime.allowRequester({
            destination,
            pathHash,
            identity,
            nowMs: this.#now(),
            entropy,
          }),
        ),
    });
  }

  announce(
    destination: DestinationHash,
    interfaceId?: InterfaceId,
  ): Promise<AnnounceOutcome> {
    return this.execute(
      Tag(
        "Announce",
        interfaceId === undefined
          ? { destination }
          : { destination, interface: interfaceId },
      ),
    );
  }

  sendSinglePacket(
    destination: DestinationHash,
    payload: Uint8Array,
  ): Promise<SendSinglePacketOutcome> {
    return this.execute(Tag("SendSinglePacket", { destination, payload }));
  }

  closeLink(value: LinkId): Promise<CloseLinkOutcome> {
    return this.execute(Tag("CloseLink", { linkId: value }));
  }

  attachInterface(
    config: InterfaceConfig,
    routing?: InterfaceRoutingPolicy,
  ): Promise<AttachOutcome> {
    return this.execute(
      routing === undefined
        ? Tag("AttachInterface", { config })
        : Tag("AttachInterface", { config, routing }),
    );
  }

  detachInterface(interfaceId: InterfaceId): Promise<DetachInterfaceOutcome> {
    return this.execute(Tag("DetachInterface", { interface: interfaceId }));
  }

  establishLink(
    destination: DestinationHash,
  ): Promise<EstablishLinkOutcome> {
    return this.execute(Tag("EstablishLink", { destination }));
  }

  requestPath(destination: DestinationHash): Promise<RequestPathOutcome> {
    return this.execute(Tag("RequestPath", { destination }));
  }

  identify(
    value: LinkId,
    identity: IdentityHash,
  ): Promise<IdentifyOutcome> {
    return this.execute(Tag("Identify", { linkId: value, identity }));
  }

  sendLinkPacket(
    value: LinkId,
    payload: Uint8Array,
  ): Promise<SendLinkPacketOutcome> {
    return this.execute(
      Tag("SendLinkPacket", { linkId: value, payload }),
    );
  }

  request(
    value: LinkId,
    pathHash: RequestPathHash,
    payload: Uint8Array,
    timeout: ResponseTimeout = Tag("LinkDefault"),
    maximumResponseBytes?: number,
  ): Promise<RequestOutcome> {
    return this.execute(
      Tag("Request", {
        linkId: value,
        pathHash,
        payload,
        timeout,
        ...(maximumResponseBytes === undefined
          ? {}
          : { maximumResponseBytes }),
      }),
    );
  }

  respond(
    value: LinkId,
    responseRequestId: RequestId,
    requestRttMillis: number,
    payload: Uint8Array,
  ): Promise<RespondOutcome> {
    return this.execute(
      Tag("Respond", {
        linkId: value,
        requestId: responseRequestId,
        requestRttMillis,
        payload,
      }),
    );
  }

  sendResource(
    value: LinkId,
    payload: Uint8Array,
    options: SendResourceOptions = {},
  ): Promise<SendResourceOutcome> {
    return this.execute(
      Tag("SendResource", {
        linkId: value,
        payload,
        compression: options.compression ?? Tag("Auto"),
        ...(options.packedMetadata === undefined
          ? {}
          : { packedMetadata: options.packedMetadata }),
      }),
    );
  }

  sendResourceBlob(
    value: LinkId,
    blob: Blob,
    options: SendResourceOptions = {},
  ): Promise<SendResourceOutcome> {
    return this.#sendResourceSource(
      value,
      blobResourceSource(blob),
      options.compression ?? Tag("Auto"),
      options.packedMetadata,
    );
  }

  setLinkResourceStrategy(
    value: LinkId,
    strategy: ResourceStrategy,
  ): Promise<SetResourceStrategyOutcome> {
    return this.execute(
      Tag("SetLinkResourceStrategy", { linkId: value, strategy }),
    );
  }

  setDestinationResourceStrategy(
    destination: DestinationHash,
    strategy: ResourceStrategy,
  ): Promise<SetResourceStrategyOutcome> {
    return this.execute(
      Tag("SetDestinationResourceStrategy", {
        destination,
        strategy,
      }),
    );
  }

  sendChannelMessage(
    value: LinkId,
    messageType: number,
    payload: Uint8Array,
  ): Promise<SendChannelMessageOutcome> {
    return this.execute(
      Tag("SendChannelMessage", {
        linkId: value,
        messageType,
        payload,
      }),
    );
  }

  allowRequester(
    destination: DestinationHash,
    pathHash: RequestPathHash,
    identity: IdentityHash,
  ): Promise<AllowRequesterOutcome> {
    return this.execute(
      Tag("AllowRequester", { destination, pathHash, identity }),
    );
  }

  get lifecycle(): HostLifecycleState {
    return this.#lifecycle;
  }

  get backendInfo(): BackendInfo {
    return cooperativeBackendInfo();
  }

  get capabilities(): BackendCapabilities {
    const info = this.backendInfo;
    return Tag("Cooperative", {
      available: new Set(info.capabilities),
      interfaceKinds: new Set(info.interfaceKinds),
    });
  }

  stop(): Promise<StopOutcome> {
    if (this.#stopCompleted) {
      return Promise.resolve(Tag("AlreadyStopped"));
    }
    if (this.#stopPromise !== undefined) {
      return this.#stopPromise;
    }
    this.#stopPromise = this.#performStop();
    return this.#stopPromise;
  }

  claimEvents(): StreamClaim<PrnsApplicationEvent> {
    this.#pumpEvents();
    return this.#events.claim();
  }

  claimDiagnostics(): StreamClaim<PrnsDiagnosticEvent> {
    this.#pumpEvents();
    return this.#diagnostics.claim();
  }

  snapshot(): SnapshotOutcome {
    try {
      return Tag("Captured", parseSnapshot(this.#runtime.snapshot()));
    } catch (error) {
      return runtimeRejected("snapshot", error);
    }
  }

  hostSnapshot(): HostSnapshotOutcome {
    try {
      const snapshot = parseSnapshot(this.#runtime.snapshot());
      const inspection = this.#host.interfaceInspection();
      const running = this.#lifecycle.tag === "Running";
      const health: InterfaceHealth = running ? "Connected" : "Disabled";
      const interfaces = snapshot.interfaces.map((entry) => {
        const active = inspection.get(byteKey(entry.id));
        return {
          interfaceId: entry.id,
          ...(active === undefined ? {} : { name: active.name }),
          ...(active?.kind === undefined ? {} : { kind: active.kind }),
          health,
          rxBytes: BigInt(active?.rxBytes ?? 0),
          txBytes: BigInt(active?.txBytes ?? 0),
          routeCount: entry.routes,
          linkCount: entry.links,
          transportedLinkCount: entry.transportedLinks,
        };
      });
      const interfaceCount = interfaces.length;
      const onlineInterfaceCount = running ? interfaceCount : 0;
      const transportedLinkCount = interfaces.reduce(
        (total, entry) =>
          saturatingAdd(total, entry.transportedLinkCount),
        0,
      );
      const rxBytes = interfaces.reduce(
        (total, entry) => total + entry.rxBytes,
        0n,
      );
      const txBytes = interfaces.reduce(
        (total, entry) => total + entry.txBytes,
        0n,
      );
      return Tag("Captured", {
        revision: snapshot.revision,
        backend: this.backendInfo,
        interfaces,
        routes: snapshot.routeSnapshots,
        activeLinkCount: snapshot.activeLinkCount,
        destinationIdentities: snapshot.destinationIdentities,
        runtime: {
          running,
          uptimeMillis: Math.max(0, this.#now() - this.#startedAtMillis),
          interfaceCount,
          onlineInterfaceCount,
          routeCount: snapshot.routeSnapshots.length,
          linkCount: snapshot.activeLinkCount,
          transportedLinkCount,
          rxBytes,
          txBytes,
          rxBps: 0,
          txBps: 0,
        },
        persistence: {
          persistent: this.#persistenceStore !== undefined,
          restored: this.#persistenceRestored,
          ...(this.#lastPersistenceFlushCause === undefined
            ? {}
            : { lastFlushCause: this.#lastPersistenceFlushCause }),
          ...(this.#persistenceFailureDetail === undefined
            ? {}
            : { lastFailureDetail: this.#persistenceFailureDetail }),
        },
      });
    } catch (error) {
      return runtimeRejected("snapshot", error);
    }
  }

  async #performStop(): Promise<StopOutcome> {
    const preserveFailure = this.#lifecycle.tag === "Failed";
    if (!preserveFailure) {
      this.#lifecycle = Tag("Stopping");
    }
    for (const pending of this.#pendingCommands.values()) {
      pending.settle(commandFailed(Tag("NodeStopped")));
    }
    this.#pendingCommands.clear();
    this.#responseParts.clear();
    const sessions = [...this.#attachedInterfaces.values()];
    this.#attachedInterfaces.clear();
    const failures = (
      await Promise.all(
        sessions.map(async (session): Promise<string | undefined> => {
          try {
            const closed = await session.close();
            return closed.tag === "Closed"
              ? undefined
              : describeInterfaceSessionFailure(closed);
          } catch (error) {
            return describeHostError(error);
          }
        }),
      )
    ).filter((failure): failure is string => failure !== undefined);
    if (this.#persistenceStore !== undefined) {
      let failure: string | undefined;
      try {
        const state = parseBrowserPersistedState(
          this.#runtime.persistedState({ nowMs: this.#now() }),
        );
        const saved = await this.#persistenceStore.save(state);
        if (saved.tag !== "Saved") {
          failure = describePersistenceStoreFailure(saved);
        }
      } catch (error) {
        failure = describeHostError(error);
      }
      if (failure === undefined) {
        this.#lastPersistenceFlushCause = "Shutdown";
        this.#persistenceFailureDetail = undefined;
        this.#diagnostics.push(
          Tag("PersistenceFlushed", {
            cause: "Shutdown",
            target: "RoutingState",
          }),
        );
        this.#diagnostics.push(
          Tag("PersistenceFlushed", {
            cause: "Shutdown",
            target: "Ratchets",
          }),
        );
      } else {
        this.#persistenceFailureDetail = failure;
        this.#diagnostics.push(
          Tag("PersistenceFlushFailed", {
            cause: "Shutdown",
            target: "RoutingState",
          }),
        );
        this.#diagnostics.push(
          Tag("PersistenceFlushFailed", {
            cause: "Shutdown",
            target: "Ratchets",
          }),
        );
        failures.push(`flush persistence: ${failure}`);
      }
    }
    this.#events.finish();
    this.#diagnostics.finish();
    this.#stopCompleted = true;
    if (failures.length > 0) {
      const detail = failures.join("; ");
      this.#lifecycle = Tag("Failed", { cause: "BackendFailed", detail });
      return Tag("OperationFailed", { operation: "stop", detail });
    }
    if (!preserveFailure) {
      this.#lifecycle = Tag("Stopped", { reason: "Requested" });
    }
    return Tag("Stopped");
  }

  #attachInterface(
    config: InterfaceConfig,
    routing: InterfaceRoutingPolicy | undefined,
  ): Promise<CommandSettlement> {
    const unsupported = async (): Promise<CommandSettlement> =>
      commandFailed(Tag("UnsupportedByBackend"));
    return match_into<Promise<CommandSettlement>>().from(config, {
      AutoLan: unsupported,
      TcpClient: unsupported,
      TcpServer: unsupported,
      Udp: unsupported,
      Serial: unsupported,
      Kiss: unsupported,
      Ax25Kiss: unsupported,
      RNode: unsupported,
      MultiRNode: unsupported,
      Pipe: unsupported,
      BackboneClient: unsupported,
      BackboneServer: unsupported,
      I2p: unsupported,
      Weave: unsupported,
      AutomaticUsb: unsupported,
      AutomaticBluetoothLe: unsupported,
      WebSocketClient: ({ target }) =>
        this.#attachWebSocket(target, "WebSocketClient", routing),
      WebSocketServer: unsupported,
      BrowserRendezvous: ({ url }) =>
        this.#attachWebSocket(url, "BrowserRendezvous", routing),
    });
  }

  async #attachWebSocket(
    target: string,
    kind: InterfaceKind,
    routing: InterfaceRoutingPolicy | undefined,
  ): Promise<CommandSettlement> {
    const connected = await this.interfaces.webSocket.connect(
      target,
      routing === undefined ? {} : { routing },
    );
    if (connected.tag !== "Connected") {
      return commandFailed(webSocketCommandFailure(connected));
    }
    const session = connected.data;
    const key = byteKey(session.interfaceId);
    if (this.#attachedInterfaces.has(key)) {
      await session.close();
      return commandFailed(
        Tag("BackendFailed", {
          detail: `runtime reused active interface identifier ${key}`,
        }),
      );
    }
    this.#host.setContractKind(session.interfaceId, kind);
    this.#attachedInterfaces.set(key, session);
    return Tag(
      "Succeeded",
      Tag("InterfaceAttached", { interface: session.interfaceId }),
    );
  }

  async #detachInterface(interfaceId: InterfaceId): Promise<CommandSettlement> {
    const key = byteKey(interfaceId);
    const session = this.#attachedInterfaces.get(key);
    if (session === undefined) {
      return commandFailed(Tag("UnknownInterface"));
    }
    this.#attachedInterfaces.delete(key);
    const closed = await session.close();
    if (closed.tag !== "Closed") {
      return commandFailed(
        Tag("BackendFailed", {
          detail: describeInterfaceSessionFailure(closed),
        }),
      );
    }
    return Tag("Succeeded", Tag("InterfaceDetached", { interface: interfaceId }));
  }

  #entropyBytes(): EntropyOutcome {
    return fillEntropy(this.#entropy, MIN_ENTROPY_BYTES);
  }

  #issueCommand(
    operation: RuntimeOperation,
    command: HostCommand,
    issue: (entropy: EntropyBytes) => bigint,
  ): Promise<CommandSettlement> {
    return this.#issuePendingCommand(
      operation,
      Tag("HostCommand", { command }),
      issue,
    );
  }

  #issueResourceSegment(
    input: RuntimeResourceSegmentIssueInput,
  ): Promise<CommandSettlement> {
    return this.#issuePendingCommand(
      "send-resource",
      Tag("ResourceSegment"),
      (entropy) =>
        this.#runtime.sendResourceSegment({
          ...input,
          nowMs: this.#now(),
          entropy,
        }),
    );
  }

  #issuePendingCommand(
    operation: RuntimeOperation,
    pending: PendingCommand,
    issue: (entropy: EntropyBytes) => bigint,
  ): Promise<CommandSettlement> {
    if (this.#lifecycle.tag !== "Running") {
      return Promise.resolve(commandFailed(Tag("NodeStopped")));
    }
    if (this.#pendingCommands.size >= this.#limits.pendingCommands) {
      return Promise.resolve(commandFailed(Tag("Busy")));
    }
    const entropy = this.#entropyBytes();
    if (entropy.tag !== "Filled") {
      return Promise.resolve(
        commandFailed(Tag("EntropyUnavailable")),
      );
    }
    let id: CommandId;
    try {
      id = commandId(issue(entropy.data));
    } catch (error) {
      return Promise.resolve(
        commandFailed(browserCommandFailure(operation, error)),
      );
    }
    return new Promise((settle) => {
      this.#pendingCommands.set(id, { pending, settle });
      this.#pumpEvents();
    });
  }

  #sendResourceSource(
    value: LinkId,
    source: ResourceSource,
    compression: ResourceCompression,
    packedMetadata: Uint8Array | undefined,
  ): Promise<ResourceSendSettlement> {
    if (this.#lifecycle.tag !== "Running") {
      return Promise.resolve(Tag("Failed", Tag("NodeStopped")));
    }
    return sendResourceFromSource(
      value,
      source,
      compression,
      packedMetadata,
      {
        maximumInFlightSegments: this.#limits.pendingCommands,
        plan: (input) => this.#runtime.resourceSegmentPlan(input),
        compress: (payload, metadata) =>
          browserResourceCompressor.compress(
            payload,
            metadata,
            this.#resourceCompressionModuleUrl,
          ),
        issue: (input) => this.#issueResourceSegment(input),
      },
    );
  }

  #pumpEvents(): void {
    if (this.#lifecycle.tag === "Failed" || this.#lifecycle.tag === "Stopped") {
      return;
    }
    let parsed: ParsedPrnsEvent[];
    try {
      parsed = this.#runtime.drainEvents().map(parseEvent);
    } catch (error) {
      this.#failContract(describeHostError(error));
      return;
    }
    for (const event of parsed) {
      match(event, {
        Application: (application) => {
          this.#events.push(application);
        },
        Diagnostic: (diagnostic) => {
          this.#diagnostics.push(diagnostic);
        },
        CommandResponse: ({ commandId: responseCommandId, event }) => {
          this.#events.push(event);
          this.#responseParts.set(responseCommandId, [event.data.data]);
        },
        CommandResponseSegment: ({
          commandId: responseCommandId,
          event,
        }) => {
          this.#events.push(event);
          const parts = this.#responseParts.get(responseCommandId) ?? [];
          parts.push(event.data.data);
          this.#responseParts.set(responseCommandId, parts);
        },
        CommandSettled: ({ commandId, settlement }) => {
          if (settlement === undefined) {
            return;
          }
          const pending = this.#pendingCommands.get(commandId);
          if (pending === undefined) {
            return;
          }
          this.#pendingCommands.delete(commandId);
          pending.settle(
            match(pending.pending, {
              HostCommand: ({ command }) =>
                this.#commandSettlement(
                  commandId,
                  command,
                  settlement,
                ),
              ResourceSegment: () => settlement,
            }),
          );
        },
      });
    }
  }

  #commandSettlement(
    id: CommandId,
    command: HostCommand,
    settlement: CommandSettlement,
  ): CommandSettlement {
    if (settlement.tag === "Failed") {
      this.#responseParts.delete(id);
      return settlement;
    }
    if (command.tag === "Request") {
      if (settlement.data.tag !== "PacketDelivered") {
        this.#responseParts.delete(id);
        return commandFailed(
          Tag("WriteFailed", {
            detail: "request settled without delivery evidence",
          }),
        );
      }
      const parts = this.#responseParts.get(id);
      this.#responseParts.delete(id);
      if (parts === undefined) {
        return commandFailed(
          Tag("WriteFailed", {
            detail: "request settled without response data",
          }),
        );
      }
      return Tag(
        "Succeeded",
        Tag("ResponseReceived", {
          data: concatenateBytes(parts),
          rttMillis: settlement.data.data.rttMillis,
        }),
      );
    }
    if (command.tag === "Respond") {
      if (settlement.data.tag !== "ResponseSent") {
        return commandFailed(
          Tag("WriteFailed", {
            detail: "response settled with an unexpected outcome",
          }),
        );
      }
      return Tag(
        "Succeeded",
        Tag("ResponseSent", {
          rttMillis: command.data.requestRttMillis,
        }),
      );
    }
    return settlement;
  }

  #failBackpressure(rejectedEventBytes: number): void {
    this.#lifecycle = Tag("Failed", {
      cause: "EventBackpressureExceeded",
      limits: this.#limits,
      rejectedEventBytes,
    });
    this.#events.finish();
    this.#diagnostics.finish();
    this.#settleFailedCommands("application event backpressure exceeded");
  }

  #failContract(detail: string): void {
    this.#lifecycle = Tag("Failed", {
      cause: "ContractViolated",
      detail,
    });
    const error = new Error(detail);
    this.#events.fail(error);
    this.#diagnostics.fail(error);
    this.#settleFailedCommands(detail);
  }

  #settleFailedCommands(detail: string): void {
    for (const pending of this.#pendingCommands.values()) {
      pending.settle(commandFailed(Tag("WriteFailed", { detail })));
    }
    this.#pendingCommands.clear();
    this.#responseParts.clear();
  }
}

class RuntimeHost {
  readonly #wasm: PrnsWasmModule;
  readonly #runtime: PrnsRuntimeBinding;
  readonly #entropy: EntropySource;
  readonly #now: () => InstantMillis;
  readonly #bleIdentityAvailability: BleIdentityAvailability;
  readonly #onRuntimeActivity: () => void;
  #activeInterfaces = new Map<
    string,
    {
      id: InterfaceId;
      name: InterfaceName;
      contractKind?: InterfaceKind;
      registrationKey: string;
      supervisorKind: RuntimeInterfaceKind;
      rxBytes: number;
      txBytes: number;
    }
  >();
  #activeRegistrationKeys = new Set<string>();
  #outboundQueues = new Map<string, PrnsOutboundFrame[]>();
  #overflowedOutbound = new Set<string>();

  constructor(
    wasm: PrnsWasmModule,
    runtime: PrnsRuntimeBinding,
    entropy: EntropySource,
    now: () => InstantMillis,
    bleIdentityAvailability: BleIdentityAvailability,
    onRuntimeActivity: () => void,
  ) {
    this.#wasm = wasm;
    this.#runtime = runtime;
    this.#entropy = entropy;
    this.#now = now;
    this.#bleIdentityAvailability = bleIdentityAvailability;
    this.#onRuntimeActivity = onRuntimeActivity;
  }

  runtimeReadiness(): RuntimeReadyOutcome {
    try {
      this.#runtime.snapshot();
      return Tag("Ready");
    } catch (error) {
      return runtimeRejected("inspect-readiness", error);
    }
  }

  registerInterface<Name extends InterfaceName>(
    registration: HostedInterfaceRegistration<Name>,
  ): InterfaceRegistrationOutcome<Name> {
    const {
      interfaceName,
      supervisorKind = registration.kind,
      contractKind = stableInterfaceKind(registration.kind),
      ...options
    } = registration;
    const registrationKey = `${options.kind}:${byteKey(options.channelTag)}`;
    if (this.#activeRegistrationKeys.has(registrationKey)) {
      return Tag("AlreadyActive", {
        interface: interfaceName,
        target: registrationKey,
      });
    }
    let id: InterfaceId;
    try {
      id = interfaceId(
        this.#runtime.registerInterface({ ...options, nowMs: this.#now() }),
      );
    } catch (error) {
      return runtimeRejected("register-interface", error);
    }
    const key = byteKey(id);
    if (this.#activeInterfaces.has(key)) {
      return Tag("AlreadyActive", {
        interface: interfaceName,
        target: key,
      });
    }
    this.#activeRegistrationKeys.add(registrationKey);
    this.#activeInterfaces.set(key, {
      id,
      name: interfaceName,
      ...(contractKind === undefined ? {} : { contractKind }),
      registrationKey,
      supervisorKind,
      rxBytes: 0,
      txBytes: 0,
    });
    this.#outboundQueues.set(key, []);
    return Tag("Registered", id);
  }

  deactivateInterface(id: InterfaceId): InterfaceDetachOutcome {
    const key = byteKey(id);
    const active = this.#activeInterfaces.get(key);
    if (!active) {
      return Tag("Detached");
    }
    try {
      const removed = this.#runtime.removeInterface({
        interfaceId: id,
        nowMs: this.#now(),
      });
      if (!removed) {
        return runtimeRejected(
          "remove-interface",
          `runtime did not contain interface ${key}`,
        );
      }
    } catch (error) {
      return runtimeRejected("remove-interface", error);
    }
    this.#activeInterfaces.delete(key);
    this.#activeRegistrationKeys.delete(active.registrationKey);
    this.#outboundQueues.delete(key);
    this.#overflowedOutbound.delete(key);
    return Tag("Detached");
  }

  setContractKind(id: InterfaceId, kind: InterfaceKind): void {
    const active = this.#activeInterfaces.get(byteKey(id));
    if (active !== undefined) {
      active.contractKind = kind;
    }
  }

  interfaceInspection(): ReadonlyMap<string, RuntimeInterfaceInspection> {
    return new Map(
      [...this.#activeInterfaces].map(([key, active]) => [
        key,
        {
          id: active.id,
          name: active.name,
          ...(active.contractKind === undefined
            ? {}
            : { kind: active.contractKind }),
          rxBytes: active.rxBytes,
          txBytes: active.txBytes,
        },
      ]),
    );
  }

  ingest(interfaceId: InterfaceId, bytes: PacketFrame): RuntimeIngestOutcome {
    const entropy = this.entropy();
    if (entropy.tag !== "Filled") {
      return entropy;
    }
    try {
      this.#runtime.ingest({
        interfaceId,
        bytes,
        nowMs: this.#now(),
        entropy: entropy.data,
      });
      const active = this.#activeInterfaces.get(byteKey(interfaceId));
      if (active !== undefined) {
        active.rxBytes = saturatingAdd(active.rxBytes, bytes.length);
      }
      this.#onRuntimeActivity();
      return Tag("Accepted");
    } catch (error) {
      return runtimeRejected("ingest", error);
    }
  }

  drainOutbound(): RuntimeOutboundDrainOutcome {
    try {
      return Tag("Drained", this.#runtime.drainOutbound().map(parseOutboundFrame));
    } catch (error) {
      return runtimeRejected("drain-outbound", error);
    }
  }

  takeOutboundFor(interfaceId: InterfaceId): OutboundTakeOutcome {
    const interfaceKey = byteKey(interfaceId);
    const direct: PrnsOutboundFrame[] = [];
    const drained = this.drainOutbound();
    if (drained.tag !== "Drained") {
      return drained;
    }
    for (const frame of drained.data) {
      for (const [key, active] of this.#activeInterfaces) {
        if (outboundTargets(frame.target, active.id, active.supervisorKind)) {
          if (key === interfaceKey) {
            direct.push(frame);
            continue;
          }
          const queue = this.#outboundQueues.get(key);
          if (queue && queue.length < INTERFACE_OUTBOUND_QUEUE_DEPTH) {
            queue.push(frame);
          } else if (queue) {
            this.#overflowedOutbound.add(key);
          }
        }
      }
    }
    if (this.#overflowedOutbound.delete(interfaceKey)) {
      this.#outboundQueues.set(interfaceKey, []);
      return Tag("OutboundQueueFull", {
        capacity: INTERFACE_OUTBOUND_QUEUE_DEPTH,
      });
    }
    const queued = this.#outboundQueues.get(interfaceKey) ?? [];
    this.#outboundQueues.set(interfaceKey, []);
    const outbound = queued.concat(direct);
    const active = this.#activeInterfaces.get(interfaceKey);
    if (active !== undefined) {
      active.txBytes = outbound.reduce(
        (total, frame) => saturatingAdd(total, frame.bytes.length),
        active.txBytes,
      );
    }
    return Tag("Outbound", outbound);
  }

  createUsbAutoDecoder(): UsbAutoDecoderBinding {
    return new this.#wasm.UsbAutoDecoder();
  }

  createBluetoothReassembler(): BluetoothReassemblerBinding {
    return new this.#wasm.BluetoothReassembler();
  }

  bluetoothServiceUuid(): string {
    return this.#wasm.bluetoothServiceUuid();
  }

  bluetoothIdentityReadiness():
    | Tag<"Ready">
    | StableIdentityUnavailable<"bluetooth"> {
    return this.#bleIdentityAvailability.tag === "Available"
      ? Tag("Ready")
      : this.#bleIdentityAvailability;
  }

  bluetoothControlUuid(): string {
    return this.#wasm.bluetoothControlUuid();
  }

  bluetoothDataUuid(): string {
    return this.#wasm.bluetoothDataUuid();
  }

  bluetoothBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.bluetoothBitrateBps());
  }

  bluetoothHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.bluetoothHardwareMtu());
  }

  bluetoothDialerHello(): Uint8Array {
    return this.#wasm.bluetoothDialerHello(this.#runtime.bluetoothIdentity());
  }

  bluetoothDecodeControl(bytes: Uint8Array): unknown {
    return this.#wasm.bluetoothDecodeControl(bytes);
  }

  bluetoothDataFragments(packet: PacketFrame): Uint8Array[] {
    return this.#wasm.bluetoothDataFragments(packet);
  }

  websocketBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.websocketBitrateBps());
  }

  websocketFrameCap(): number {
    return positiveInteger(this.#wasm.websocketFrameCap(), "WebSocket frame cap");
  }

  websocketHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.websocketHardwareMtu());
  }

  autoWifiReady(): RuntimeReadyOutcome {
    return this.runtimeReadiness();
  }

  autoWifiRegister(id: Uint8Array): InterfaceRegistrationOutcome<"auto-wifi"> {
    try {
      return this.registerInterface({
        interfaceName: "auto-wifi",
        kind: "auto-wifi",
        channelTag: channelTag(id),
        bitrateBps: this.websocketBitrateBps(),
        hardwareMtu: this.websocketHardwareMtu(),
      });
    } catch (error) {
      return runtimeRejected("register-interface", error);
    }
  }

  autoWifiDeactivate(id: InterfaceId): InterfaceDetachOutcome {
    return this.deactivateInterface(id);
  }

  autoWifiIngest(id: InterfaceId, bytes: Uint8Array): RuntimeIngestOutcome {
    try {
      return this.ingest(id, packetFrame(bytes));
    } catch (error) {
      return runtimeRejected("ingest", error);
    }
  }

  autoWifiTakeOutbound(id: InterfaceId): OutboundTakeOutcome {
    return this.takeOutboundFor(id);
  }

  autoWifiBitrateBps(): BitrateBps {
    return this.websocketBitrateBps();
  }

  autoWifiHardwareMtu(): HardwareMtu {
    return this.websocketHardwareMtu();
  }

  autoWifiFrameCap(): number {
    return this.websocketFrameCap();
  }

  usbAutoHostBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.usbAutoHostBitrateBps());
  }

  usbAutoHostHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.usbAutoHostHardwareMtu());
  }

  defaultUsbAutoFilters(): readonly BrowserUsbDeviceFilter[] {
    return [
      {
        vendorId: this.#wasm.usbAutoWebUsbVendorId(),
        productId: this.#wasm.usbAutoWebUsbProductId(),
      },
    ];
  }

  usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array {
    return this.#wasm.usbAutoNodeTagFor(interfaceId);
  }

  usbAutoHostHelloFrame(): Uint8Array {
    return this.#wasm.usbAutoHostHelloFrame();
  }

  usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array {
    return this.#wasm.usbAutoHostHelloAckFrame(nodeTag);
  }

  usbAutoDataFrame(packet: PacketFrame): Uint8Array {
    return this.#wasm.usbAutoDataFrame(packet);
  }

  entropy(): EntropyOutcome {
    return fillEntropy(this.#entropy, MIN_ENTROPY_BYTES);
  }
}

export function identitySecretKey(
  bytes: Uint8Array,
  expectedLength: number,
): IdentitySecretKey {
  return exactBytes(bytes, expectedLength, "IdentitySecretKey") as IdentitySecretKey;
}

export function bleIdentity(bytes: Uint8Array): BleIdentityValidationOutcome {
  return bytes.length === BLE_IDENTITY_LENGTH
    ? Tag("ValidBleIdentity", copyBytes(bytes) as BleIdentity)
    : Tag("InvalidBleIdentity", { actualLength: bytes.length });
}

export function channelTag(bytes: Uint8Array): ChannelTag {
  return nonEmptyBytes(bytes, "ChannelTag") as ChannelTag;
}

export function packetFrame(bytes: Uint8Array): PacketFrame {
  return nonEmptyBytes(bytes, "PacketFrame") as PacketFrame;
}

export function entropyBytes(bytes: Uint8Array): EntropyBytes {
  if (bytes.length < MIN_ENTROPY_BYTES) {
    throw new PrnsValidationError(
      "invalid-length",
      `EntropyBytes requires at least ${MIN_ENTROPY_BYTES} bytes`,
    );
  }
  return copyBytes(bytes) as EntropyBytes;
}

export function appData(bytes: Uint8Array = new Uint8Array()): AppData {
  return copyBytes(bytes) as AppData;
}

export function appName(value: string): AppName {
  return dottedComponent(value, "AppName") as AppName;
}

export function aspect(value: string): Aspect {
  return dottedComponent(value, "Aspect") as Aspect;
}

export function bitrateBps(value: number): BitrateBps {
  return positiveInteger(value, "BitrateBps") as BitrateBps;
}

export function hardwareMtu(value: number): HardwareMtu {
  return positiveInteger(value, "HardwareMtu") as HardwareMtu;
}

export function hopCount(value: number): HopCount {
  if (!Number.isInteger(value) || value < 0 || value > 255) {
    throw new PrnsValidationError(
      "invalid-number",
      "HopCount must be an integer from 0 through 255",
    );
  }
  return value as HopCount;
}

export function nowMillis(value: number = Date.now()): InstantMillis {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new PrnsValidationError(
      "invalid-number",
      "InstantMillis must be a non-negative safe integer",
    );
  }
  return value as InstantMillis;
}

export function commandId(value: bigint): CommandId {
  if (value < 0n) {
    throw new PrnsValidationError(
      "invalid-number",
      "CommandId must be non-negative",
    );
  }
  return value as CommandId;
}

export function webCryptoEntropy(length: number): EntropyOutcome {
  try {
    if (!hostGlobal().crypto) {
      return Tag("HostApiUnavailable", { api: "Crypto" });
    }
    const bytes = webCryptoBytes(length);
    if (bytes.length < MIN_ENTROPY_BYTES) {
      return Tag("InsufficientEntropy", {
        minimum: MIN_ENTROPY_BYTES,
        actual: bytes.length,
      });
    }
    return Tag("Filled", bytes as EntropyBytes);
  } catch (error) {
    return Tag("EntropySourceFailed", { detail: describeHostError(error) });
  }
}

function outboundTargets(
  target: OutboundTarget,
  interfaceId: InterfaceId,
  supervisorKind: RuntimeInterfaceKind,
): boolean {
  return match_into<boolean>().from(target, {
    Interface: (targetInterface) =>
      equalBytes(targetInterface, interfaceId),
    Broadcast: ({ supervisorKind: targetKind, fan }) =>
      targetKind === supervisorKind &&
      match_into<boolean>().from(fan, {
        All: () => true,
        Only: (targetInterface) =>
          equalBytes(targetInterface, interfaceId),
        AllExcept: (targetInterface) =>
          !equalBytes(targetInterface, interfaceId),
      }),
  });
}

type RawUsbAutoMessageType =
  | "hello"
  | "helloAck"
  | "data"
  | "configRequest"
  | "configResponse"
  | "snapshot";

const RAW_USB_AUTO_MESSAGE_TYPES: ReadonlySet<string> = new Set<
  RawUsbAutoMessageType
>(["hello", "helloAck", "data", "configRequest", "configResponse", "snapshot"]);

const USB_AUTO_CONFIG_RESULTS: ReadonlySet<string> = new Set<
  UsbAutoConfigResult
>(["ok", "applyFailed", "profileNotSaved", "rejected", "badPayload"]);

function parseUsbAutoMessage(raw: unknown): UsbAutoInboundMessage {
  const object = record(raw, "UsbAutoInboundMessage");
  const type = stringField(object, "type");
  if (!RAW_USB_AUTO_MESSAGE_TYPES.has(type)) {
    throw new PrnsValidationError(
      "invalid-component",
      `unknown USB-auto message ${type}`,
    );
  }
  return match(type as RawUsbAutoMessageType, {
    hello: () => Tag("Hello"),
    helloAck: () => Tag("HelloAck", bytesField(object, "tag")),
    data: () => Tag("Data", bytesField(object, "bytes")),
    configRequest: () =>
      Tag("ConfigRequest", {
        requestId: numberField(object, "requestId"),
        action: bytesField(object, "action"),
      }),
    configResponse: () => {
      const result = stringField(object, "result") as UsbAutoConfigResult;
      if (!USB_AUTO_CONFIG_RESULTS.has(result)) {
        throw new PrnsValidationError(
          "invalid-component",
          `unknown USB-auto config result ${result}`,
        );
      }
      return Tag("ConfigResponse", {
        requestId: numberField(object, "requestId"),
        result,
        detail: bytesField(object, "detail"),
      });
    },
    snapshot: () =>
      Tag("Snapshot", {
        schemaVersion: numberField(object, "schemaVersion"),
        body: bytesField(object, "body"),
      }),
  });
}

type RawBluetoothControlType = "hello" | "welcome" | "close";

const RAW_BLUETOOTH_CONTROL_TYPES: ReadonlySet<string> =
  new Set<RawBluetoothControlType>(["hello", "welcome", "close"]);

function parseBluetoothControl(raw: unknown): BluetoothControl {
  const object = record(raw, "BluetoothControl");
  const type = stringField(object, "type");
  if (!RAW_BLUETOOTH_CONTROL_TYPES.has(type)) {
    throw new PrnsValidationError(
      "invalid-component",
      `unknown Bluetooth control ${type}`,
    );
  }
  return match(type as RawBluetoothControlType, {
    hello: () => Tag("Hello", bytesField(object, "identity")),
    welcome: () => Tag("Welcome", bytesField(object, "identity")),
    close: () => Tag("Close", stringField(object, "reason")),
  });
}

function parseOutboundFrame(raw: unknown): PrnsOutboundFrame {
  const object = record(raw, "PrnsOutboundFrame");
  const type = stringField(object, "type");
  if (type !== "frame" && type !== "announce") {
    throw new PrnsValidationError(
      "unknown-outbound-target",
      `unknown outbound frame type ${type}`,
    );
  }
  const frame: PrnsOutboundFrame = {
    type,
    target: parseOutboundTarget(field(object, "target")),
    bytes: packetFrame(bytesField(object, "bytes")),
  };
  const hops = optionalNumber(object, "hops", hopCount);
  if (hops !== undefined) {
    frame.hops = hops;
  }
  return frame;
}

function parseOutboundTarget(raw: unknown): OutboundTarget {
  const object = record(raw, "OutboundTarget");
  const type = stringField(object, "type");
  if (type === "interface") {
    return Tag(
      "Interface",
      interfaceId(bytesField(object, "interfaceId")),
    );
  }
  if (type === "broadcast") {
    return Tag("Broadcast", {
      supervisorKind: parseRuntimeInterfaceKind(stringField(object, "supervisorKind")),
      fan: parseFanTarget(field(object, "fan")),
    });
  }
  throw new PrnsValidationError(
    "unknown-outbound-target",
    `unknown outbound target ${type}`,
  );
}

function parseFanTarget(raw: unknown): FanTarget {
  const object = record(raw, "FanTarget");
  const type = stringField(object, "type");
  if (type === "all") {
    return Tag("All");
  }
  if (type === "only") {
    return Tag(
      "Only",
      interfaceId(bytesField(object, "interfaceId")),
    );
  }
  if (type === "allExcept") {
    return Tag(
      "AllExcept",
      interfaceId(bytesField(object, "interfaceId")),
    );
  }
  throw new PrnsValidationError(
    "unknown-outbound-target",
    `unknown fan target ${type}`,
  );
}

function parseEvent(raw: unknown): ParsedPrnsEvent {
  const object = record(raw, "PrnsEvent");
  const event = Tag(
    rawEventType(stringField(object, "type")),
    object,
  ) as RawEvent;
  return match_into<ParsedPrnsEvent>().from(event, {
    announce: (data) =>
      Tag(
        "Diagnostic",
        Tag("AnnounceHeard", {
          destination: destinationHash(bytesField(data, "destination")),
          hops: hopCount(numberField(data, "hops")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        }),
      ),
    selfRatchetRotated: (data) =>
      Tag(
        "Diagnostic",
        Tag("SelfRatchetRotated", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    announceHeldDropped: (data) =>
      Tag(
        "Diagnostic",
        Tag("AnnounceHeldDropped", {
          destination: destinationHash(bytesField(data, "destination")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
          cause: stringField(data, "cause"),
        }),
      ),
    commandSettled: (data) => {
      const commandIdValue = commandId(bigintField(data, "id"));
      const settlement = parseCommandSettlement(data);
      return Tag(
        "CommandSettled",
        settlement === undefined
          ? { commandId: commandIdValue }
          : { commandId: commandIdValue, settlement },
      );
    },
    linkEstablished: (data) =>
      Tag(
        "Diagnostic",
        Tag("LinkEstablished", {
          linkId: linkId(bytesField(data, "linkId")),
          rttMillis: nonNegativeInteger(
            numberField(data, "rttMillis"),
            "rttMillis",
          ),
        }),
      ),
    peerIdentified: (data) =>
      Tag(
        "Diagnostic",
        Tag("PeerIdentified", {
          linkId: linkId(bytesField(data, "linkId")),
          identity: identityHash(bytesField(data, "identity")),
        }),
      ),
    request: (data) => {
      const request = {
        destination: destinationHash(bytesField(data, "destination")),
        linkId: linkId(bytesField(data, "linkId")),
        requestId: requestId(bytesField(data, "requestId")),
        pathHash: requestPathHash(bytesField(data, "pathHash")),
        rttMillis: nonNegativeInteger(
          numberField(data, "rttMillis"),
          "rttMillis",
        ),
        data: copyBytes(bytesField(data, "data")),
      };
      const requester = optionalBytesField(data, "requester");
      return Tag(
        "Application",
        Tag(
          "Request",
          requester
            ? { ...request, requester: identityHash(requester) }
            : request,
        ),
      );
    },
    response: (data) => {
      const responseCommandId = commandId(bigintField(data, "commandId"));
      return Tag(
        "CommandResponse",
        {
          commandId: responseCommandId,
          event: Tag("Response", {
            linkId: linkId(bytesField(data, "linkId")),
            requestId: requestId(bytesField(data, "requestId")),
            data: copyBytes(bytesField(data, "data")),
          }),
        },
      );
    },
    responseSegment: (data) => {
      const responseCommandId = commandId(bigintField(data, "commandId"));
      return Tag(
        "CommandResponseSegment",
        {
          commandId: responseCommandId,
          event: Tag("ResponseSegment", {
            linkId: linkId(bytesField(data, "linkId")),
            requestId: requestId(bytesField(data, "requestId")),
            segmentIndex: nonNegativeInteger(
              numberField(data, "segmentIndex"),
              "segmentIndex",
            ),
            totalSegments: positiveInteger(
              numberField(data, "totalSegments"),
              "totalSegments",
            ),
            data: copyBytes(bytesField(data, "data")),
          }),
        },
      );
    },
    channelMessage: (data) =>
      Tag(
        "Application",
        Tag("ChannelMessage", {
          linkId: linkId(bytesField(data, "linkId")),
          messageType: nonNegativeInteger(
            numberField(data, "messageType"),
            "messageType",
          ),
          data: copyBytes(bytesField(data, "data")),
        }),
      ),
    singleDelivery: (data) =>
      Tag(
        "Application",
        Tag("SingleDelivery", {
          destination: destinationHash(bytesField(data, "destination")),
          plaintext: copyBytes(bytesField(data, "plaintext")),
          sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        }),
      ),
    delivered: (data) =>
      Tag(
        "Diagnostic",
        Tag("Delivered", { detail: stringField(data, "detail") }),
      ),
    linkClosed: (data) =>
      Tag(
        "Diagnostic",
        Tag("LinkClosed", {
          linkId: linkId(bytesField(data, "linkId")),
          reason: linkClosedReason(stringField(data, "reason")),
        }),
      ),
    linkInterfaceMismatch: (data) =>
      Tag(
        "Diagnostic",
        Tag("LinkInterfaceMismatch", {
          linkId: linkId(bytesField(data, "linkId")),
          attachedInterface: interfaceId(
            bytesField(data, "attachedInterface"),
          ),
          arrivedOn: interfaceId(bytesField(data, "arrivedOn")),
        }),
      ),
    resourceReceived: (data) => {
      const details = {
        linkId: linkId(bytesField(data, "linkId")),
        hash: resourceHash(bytesField(data, "hash")),
        resource: new MemoryResourceStream(bytesField(data, "data")),
      };
      const metadata = optionalBytesField(data, "metadata");
      return Tag(
        "Application",
        Tag(
          "ResourceAvailable",
          metadata
            ? { ...details, metadata: copyBytes(metadata) }
            : details,
        ),
      );
    },
    resourceFailed: (data) =>
      Tag(
        "Diagnostic",
        Tag("ResourceFailed", {
          linkId: linkId(bytesField(data, "linkId")),
          hash: resourceHash(bytesField(data, "hash")),
          cause: stringField(data, "cause"),
        }),
      ),
    resourceNeedsDecompression: (data) =>
      Tag(
        "Application",
        Tag("ResourceNeedsDecompression", {
          linkId: linkId(bytesField(data, "linkId")),
          hash: resourceHash(bytesField(data, "hash")),
          stream: copyBytes(bytesField(data, "stream")),
          uncompressedDataBytes: nonNegativeBigIntField(
            data,
            "uncompressedDataBytes",
          ),
        }),
      ),
    resourceSegment: (data) => {
      const details = {
        linkId: linkId(bytesField(data, "linkId")),
        originalHash: resourceHash(bytesField(data, "originalHash")),
        segmentIndex: nonNegativeInteger(
          numberField(data, "segmentIndex"),
          "segmentIndex",
        ),
        totalSegments: positiveInteger(
          numberField(data, "totalSegments"),
          "totalSegments",
        ),
        data: copyBytes(bytesField(data, "data")),
      };
      const metadata = optionalBytesField(data, "metadata");
      return Tag(
        "Application",
        Tag(
          "ResourceSegment",
          metadata
            ? { ...details, metadata: copyBytes(metadata) }
            : details,
        ),
      );
    },
    resourceAssembled: (data) =>
      Tag(
        "Diagnostic",
        Tag("ResourceAssembled", {
          linkId: linkId(bytesField(data, "linkId")),
          originalHash: resourceHash(bytesField(data, "originalHash")),
          totalSizeBytes: nonNegativeBigIntField(data, "totalSizeBytes"),
        }),
      ),
    routeExpired: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteExpired", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    routeEvicted: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteEvicted", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    routeInterfaceGone: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteInterfaceGone", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
    routeDropped: (data) =>
      Tag(
        "Diagnostic",
        Tag("RouteDropped", {
          destination: destinationHash(bytesField(data, "destination")),
        }),
      ),
  });
}

function parseCommandSettlement(
  value: Record<string, unknown>,
): CommandSettlement | undefined {
  const result = stringField(value, "result");
  if (result === "untracked") {
    return undefined;
  }
  if (result === "failed") {
    return commandFailed(parseCommandFailure(value));
  }
  if (result !== "succeeded") {
    throw new PrnsValidationError(
      "invalid-component",
      `unknown command settlement result ${result}`,
    );
  }
  const kind = stringField(value, "kind");
  if (kind === "Announced") {
    return Tag("Succeeded", Tag("Announced"));
  }
  if (kind === "LinkCloseQueued") {
    return Tag("Succeeded", Tag("LinkCloseQueued"));
  }
  if (kind === "PacketDelivered") {
    const delivered = {
      rttMillis: nonNegativeInteger(
        numberField(value, "rttMillis"),
        "rttMillis",
      ),
      evidence: parseDeliveryEvidence(stringField(value, "evidence")),
    };
    const hash = optionalBytesField(value, "packetHash");
    return Tag(
      "Succeeded",
      Tag(
        "PacketDelivered",
        hash === undefined
          ? delivered
          : { ...delivered, packetHash: packetHash(hash) },
      ),
    );
  }
  if (kind === "LinkEstablished") {
    return Tag(
      "Succeeded",
      Tag("LinkEstablished", {
        linkId: linkId(bytesField(value, "linkId")),
        rttMillis: nonNegativeInteger(
          numberField(value, "rttMillis"),
          "rttMillis",
        ),
      }),
    );
  }
  if (kind === "PathDiscovered") {
    return Tag(
      "Succeeded",
      Tag("PathDiscovered", {
        hops: nonNegativeInteger(numberField(value, "hops"), "hops"),
      }),
    );
  }
  if (kind === "Identified") {
    return Tag("Succeeded", Tag("Identified"));
  }
  if (kind === "ResponseSent") {
    return Tag(
      "Succeeded",
      Tag("ResponseSent", {
        rttMillis: nonNegativeInteger(
          numberField(value, "rttMillis"),
          "rttMillis",
        ),
      }),
    );
  }
  if (kind === "ResourceSent") {
    return Tag("Succeeded", Tag("ResourceSent"));
  }
  if (kind === "ResourceStrategySet") {
    return Tag("Succeeded", Tag("ResourceStrategySet"));
  }
  if (kind === "RequesterAllowed") {
    return Tag("Succeeded", Tag("RequesterAllowed"));
  }
  throw new PrnsValidationError(
    "invalid-component",
    `unknown command outcome ${kind}`,
  );
}

function parseCommandFailure(value: Record<string, unknown>): CommandFailure {
  const kind = stringField(value, "kind");
  if (kind === "NodeStopped") {
    return Tag("NodeStopped");
  }
  if (kind === "Busy") {
    return Tag("Busy");
  }
  if (kind === "PayloadTooLarge") {
    return Tag("PayloadTooLarge");
  }
  if (kind === "ResponseTooLarge") {
    return Tag("ResponseTooLarge");
  }
  if (kind === "UnknownDestination") {
    return Tag("UnknownDestination");
  }
  if (kind === "NotSingleDestination") {
    return Tag("NotSingleDestination");
  }
  if (kind === "AnnounceAppDataTooLong") {
    return Tag("AnnounceAppDataTooLong");
  }
  if (kind === "UnknownInterface") {
    return Tag("UnknownInterface");
  }
  if (kind === "NoRouteToDestination") {
    return Tag("NoRouteToDestination");
  }
  if (kind === "NotDirectlyReachable") {
    return Tag("NotDirectlyReachable");
  }
  if (kind === "PacketCulled") {
    return Tag("PacketCulled");
  }
  if (kind === "DeliveryTimedOut") {
    return Tag("DeliveryTimedOut");
  }
  if (kind === "InvalidBitrate") {
    return Tag("InvalidBitrate");
  }
  if (kind === "BindFailed") {
    return Tag("BindFailed", { detail: stringField(value, "detail") });
  }
  if (kind === "WriteFailed") {
    return Tag("WriteFailed", { detail: stringField(value, "detail") });
  }
  if (kind === "UnsupportedByBackend") {
    return Tag("UnsupportedByBackend");
  }
  if (kind === "UnknownLink") {
    return Tag("UnknownLink");
  }
  if (kind === "LinkNotActive") {
    return Tag("LinkNotActive");
  }
  if (kind === "EntropyUnavailable") {
    return Tag("EntropyUnavailable");
  }
  if (kind === "NotLinkInitiator") {
    return Tag("NotLinkInitiator");
  }
  if (kind === "IdentityNotHeld") {
    return Tag("IdentityNotHeld");
  }
  if (kind === "UnknownRequestHandler") {
    return Tag("UnknownRequestHandler");
  }
  if (kind === "RequestPolicyNotAllowList") {
    return Tag("RequestPolicyNotAllowList");
  }
  if (kind === "RequestAllowListFull") {
    return Tag("RequestAllowListFull");
  }
  if (kind === "LinkBusy") {
    return Tag("LinkBusy");
  }
  if (kind === "ResourceTableFull") {
    return Tag("ResourceTableFull");
  }
  if (kind === "ResourceMetadataTooLarge") {
    return Tag("ResourceMetadataTooLarge");
  }
  if (kind === "ResourceRejectedByPeer") {
    return Tag("ResourceRejectedByPeer");
  }
  if (kind === "ResourceSequencingFailed") {
    return Tag("ResourceSequencingFailed");
  }
  if (kind === "ResourcePredecessorFailed") {
    return Tag("ResourcePredecessorFailed");
  }
  if (kind === "ChannelWindowFull") {
    return Tag("ChannelWindowFull");
  }
  if (kind === "ChannelUntrackable") {
    return Tag("ChannelUntrackable");
  }
  if (kind === "InvalidChannelMessageType") {
    return Tag("InvalidChannelMessageType");
  }
  throw new PrnsValidationError(
    "invalid-component",
    `unknown command failure ${kind}`,
  );
}

function parseDeliveryEvidence(value: string): DeliveryEvidenceKind {
  if (
    value === "ExplicitProof" ||
    value === "ImplicitProof" ||
    value === "Response"
  ) {
    return value;
  }
  throw new PrnsValidationError(
    "invalid-component",
    `unknown delivery evidence ${value}`,
  );
}

function parseSnapshot(raw: unknown): PrnsSnapshot {
  const object = record(raw, "PrnsSnapshot");
  const interfacesRaw = field(object, "interfaces");
  if (!Array.isArray(interfacesRaw)) {
    throw new PrnsValidationError(
      "invalid-component",
      "snapshot interfaces must be an array",
    );
  }
  const routeSnapshotsRaw = optionalArrayField(object, "routeSnapshots");
  const destinationIdentitiesRaw = optionalArrayField(
    object,
    "destinationIdentities",
  );
  return {
    type: literalField(object, "type", "snapshot"),
    revision: nonNegativeBigIntField(object, "revision"),
    ingestedPackets: nonNegativeInteger(
      numberField(object, "ingestedPackets"),
      "ingestedPackets",
    ),
    ingestedCommands: nonNegativeInteger(
      numberField(object, "ingestedCommands"),
      "ingestedCommands",
    ),
    routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
    scheduledAnnounces: nonNegativeInteger(
      numberField(object, "scheduledAnnounces"),
      "scheduledAnnounces",
    ),
    interfaces: interfacesRaw.map(parseInterfaceSnapshot),
    activeLinkCount: optionalNumber(
      object,
      "activeLinkCount",
      (value) => nonNegativeInteger(value, "activeLinkCount"),
    ) ?? 0,
    routeSnapshots: routeSnapshotsRaw.map(parseStableRouteSnapshot),
    destinationIdentities: destinationIdentitiesRaw.map(
      parseStableDestinationIdentitySnapshot,
    ),
  };
}

function parseInterfaceSnapshot(raw: unknown): InterfaceSnapshot {
  const object = record(raw, "InterfaceSnapshot");
  const snapshot: InterfaceSnapshot = {
    id: interfaceId(bytesField(object, "id")),
    kind: stringField(object, "kind"),
    routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
    links: nonNegativeInteger(numberField(object, "links"), "links"),
    transportedLinks: optionalNumber(
      object,
      "transportedLinks",
      (value) => nonNegativeInteger(value, "transportedLinks"),
    ) ?? 0,
  };
  const bitrate = optionalNumber(object, "bitrateBps", bitrateBps);
  if (bitrate !== undefined) {
    snapshot.bitrateBps = bitrate;
  }
  const mtu = optionalNumber(object, "hardwareMtu", hardwareMtu);
  if (mtu !== undefined) {
    snapshot.hardwareMtu = mtu;
  }
  return snapshot;
}

function parseStableRouteSnapshot(raw: unknown): StableRouteSnapshot {
  const object = record(raw, "RouteSnapshot");
  const viaIdentity = optionalBytesField(object, "viaIdentity");
  return {
    destination: destinationHash(bytesField(object, "destination")),
    hops: nonNegativeInteger(numberField(object, "hops"), "hops"),
    ...(viaIdentity === undefined
      ? {}
      : { viaIdentity: identityHash(viaIdentity) }),
    interfaceId: interfaceId(bytesField(object, "interfaceId")),
    learnedAtMillis: nonNegativeInteger(
      numberField(object, "learnedAtMillis"),
      "learnedAtMillis",
    ),
    lastRelayedAtMillis: nonNegativeInteger(
      numberField(object, "lastRelayedAtMillis"),
      "lastRelayedAtMillis",
    ),
    expiresAtMillis: nonNegativeInteger(
      numberField(object, "expiresAtMillis"),
      "expiresAtMillis",
    ),
  };
}

function parseStableDestinationIdentitySnapshot(
  raw: unknown,
): StableDestinationIdentitySnapshot {
  const object = record(raw, "DestinationIdentitySnapshot");
  return {
    destination: destinationHash(bytesField(object, "destination")),
    identity: identityHash(bytesField(object, "identity")),
  };
}

function parseRuntimeInterfaceKind(value: string): RuntimeInterfaceKind {
  if (
    value === "auto-usb-host" ||
    value === "auto-usb-device" ||
    value === "rnode" ||
    value === "bluetooth-auto" ||
    value === "bluetooth-peer" ||
    value === "auto-wifi" ||
    value === "websocket-client" ||
    value === "websocket-server" ||
    value === "websocket-server-peer" ||
    value === "serial" ||
    value === "kiss" ||
    value === "pipe"
  ) {
    return value;
  }
  throw new PrnsValidationError(
    "unknown-interface-kind",
    `unknown interface kind ${value}`,
  );
}

function exactBytes(
  bytes: Uint8Array,
  expectedLength: number,
  name: string,
): Uint8Array {
  if (bytes.length !== expectedLength) {
    throw new PrnsValidationError(
      "invalid-length",
      `${name} must be ${expectedLength} bytes`,
    );
  }
  return copyBytes(bytes);
}

function nonEmptyBytes(bytes: Uint8Array, name: string): Uint8Array {
  if (bytes.length === 0) {
    throw new PrnsValidationError("empty-bytes", `${name} must not be empty`);
  }
  return copyBytes(bytes);
}

function copyBytes(bytes: Uint8Array): Uint8Array {
  return new Uint8Array(bytes);
}

function dottedComponent(value: string, name: string): string {
  if (value.length === 0) {
    throw new PrnsValidationError("empty-string", `${name} must not be empty`);
  }
  if (value.includes(".")) {
    throw new PrnsValidationError(
      "invalid-component",
      `${name} must not contain dots`,
    );
  }
  return value;
}

function positiveInteger(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} must be a positive safe integer`,
    );
  }
  return value;
}

function nonNegativeInteger(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} must be a non-negative safe integer`,
    );
  }
  return value;
}

function runtimeInterfaceRouting(
  routing: InterfaceRoutingPolicy | undefined,
): Pick<
  RuntimeRegisterInterfaceOptions,
  | "mode"
  | "gravity"
  | "recursivePathRequests"
  | "announcesFromInternal"
  | "announcesToInternal"
> {
  if (routing === undefined) return {};
  if (routing.gravity !== undefined && !Number.isSafeInteger(routing.gravity)) {
    throw new PrnsValidationError(
      "invalid-number",
      "gravity must be a safe integer",
    );
  }
  return {
    ...(routing.mode === undefined ? {} : { mode: routing.mode }),
    ...(routing.gravity === undefined ? {} : { gravity: routing.gravity }),
    ...(routing.recursivePathRequests === undefined
      ? {}
      : { recursivePathRequests: routing.recursivePathRequests }),
    ...(routing.announcesFromInternal === undefined
      ? {}
      : { announcesFromInternal: routing.announcesFromInternal }),
    ...(routing.announcesToInternal === undefined
      ? {}
      : { announcesToInternal: routing.announcesToInternal }),
  };
}

function field(object: Record<string, unknown>, key: string): unknown {
  if (!(key in object)) {
    throw new PrnsValidationError(
      "invalid-component",
      `missing field ${key}`,
    );
  }
  return object[key];
}

function stringField(object: Record<string, unknown>, key: string): string {
  const value = field(object, key);
  if (typeof value !== "string") {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a string`,
    );
  }
  return value;
}

function literalField<T extends string>(
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

function numberField(object: Record<string, unknown>, key: string): number {
  const value = field(object, key);
  if (typeof value !== "number") {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a number`,
    );
  }
  return value;
}

function nonNegativeBigIntField(
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

function optionalNumber<T>(
  object: Record<string, unknown>,
  key: string,
  parse: (value: number) => T,
): T | undefined {
  if (!(key in object)) {
    return undefined;
  }
  return parse(numberField(object, key));
}

function optionalBytesField(
  object: Record<string, unknown>,
  key: string,
): Uint8Array | undefined {
  return key in object ? bytesField(object, key) : undefined;
}

function optionalArrayField(
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

function bigintField(object: Record<string, unknown>, key: string): bigint {
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

function bytesField(object: Record<string, unknown>, key: string): Uint8Array {
  const value = field(object, key);
  if (!(value instanceof Uint8Array)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${key} must be a Uint8Array`,
    );
  }
  return value;
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `${name} must be an object`,
    );
  }
  return value as Record<string, unknown>;
}

function webCryptoBytes(length: number): Uint8Array {
  if (!Number.isSafeInteger(length) || length <= 0) {
    throw new PrnsValidationError(
      "invalid-number",
      "random byte length must be a positive safe integer",
    );
  }
  const out = new Uint8Array(length);
  const crypto = hostGlobal().crypto;
  if (!crypto) {
    throw new PrnsValidationError(
      "missing-host-api",
      "Prns entropy requires globalThis.crypto.getRandomValues",
    );
  }
  crypto.getRandomValues(out);
  return out;
}

function encodeBase64(bytes: Uint8Array): string {
  const btoa = hostGlobal().btoa;
  if (!btoa) {
    throw new PrnsValidationError(
      "missing-host-api",
      "BrowserLocalStorageIdentityStore requires globalThis.btoa",
    );
  }
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

function decodeBase64(encoded: string): Uint8Array {
  const atob = hostGlobal().atob;
  if (!atob) {
    throw new PrnsValidationError(
      "missing-host-api",
      "BrowserLocalStorageIdentityStore requires globalThis.atob",
    );
  }
  const binary = atob(encoded);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i);
  }
  return out;
}

function parseBrowserPersistedState(value: unknown): BrowserPersistedState {
  const object = record(value, "browser persisted state");
  const persistenceVersion = nonNegativeInteger(
    numberField(object, "persistenceVersion"),
    "persisted state version",
  );
  if (persistenceVersion !== BROWSER_PERSISTENCE_VERSION) {
    throw new PrnsValidationError(
      "invalid-component",
      `persisted state version ${persistenceVersion} does not match ${BROWSER_PERSISTENCE_VERSION}`,
    );
  }
  const rawRatchets = field(object, "ratchets");
  if (!Array.isArray(rawRatchets)) {
    throw new PrnsValidationError(
      "invalid-component",
      "persisted state ratchets must be an array",
    );
  }
  return {
    type: literalField(object, "type", "persistedState"),
    persistenceVersion,
    takenAtMillis: nowMillis(
      nonNegativeInteger(
        numberField(object, "takenAtMillis"),
        "persisted state timestamp",
      ),
    ),
    routingTable: new Uint8Array(bytesField(object, "routingTable")),
    tunnels: new Uint8Array(bytesField(object, "tunnels")),
    destinationIdentities: new Uint8Array(
      bytesField(object, "destinationIdentities"),
    ),
    ratchets: rawRatchets.map((raw) => {
      const ratchet = record(raw, "persisted ratchet");
      return {
        destination: destinationHash(bytesField(ratchet, "destination")),
        sealed: new Uint8Array(bytesField(ratchet, "sealed")),
      };
    }),
  };
}

function encodeBrowserPersistedState(state: BrowserPersistedState): string {
  const parsed = parseBrowserPersistedState(state);
  return JSON.stringify({
    type: parsed.type,
    persistenceVersion: parsed.persistenceVersion,
    takenAtMillis: parsed.takenAtMillis,
    routingTable: encodeBase64(parsed.routingTable),
    tunnels: encodeBase64(parsed.tunnels),
    destinationIdentities: encodeBase64(parsed.destinationIdentities),
    ratchets: parsed.ratchets.map(({ destination, sealed }) => ({
      destination: encodeBase64(destination),
      sealed: encodeBase64(sealed),
    })),
  });
}

function decodeBrowserPersistedState(encoded: string): BrowserPersistedState {
  const stored = record(JSON.parse(encoded), "stored browser persistence");
  const rawRatchets = field(stored, "ratchets");
  if (!Array.isArray(rawRatchets)) {
    throw new PrnsValidationError(
      "invalid-component",
      "stored persistence ratchets must be an array",
    );
  }
  return parseBrowserPersistedState({
    type: stringField(stored, "type"),
    persistenceVersion: numberField(stored, "persistenceVersion"),
    takenAtMillis: numberField(stored, "takenAtMillis"),
    routingTable: decodeBase64(stringField(stored, "routingTable")),
    tunnels: decodeBase64(stringField(stored, "tunnels")),
    destinationIdentities: decodeBase64(
      stringField(stored, "destinationIdentities"),
    ),
    ratchets: rawRatchets.map((raw) => {
      const ratchet = record(raw, "stored persisted ratchet");
      return {
        destination: decodeBase64(stringField(ratchet, "destination")),
        sealed: decodeBase64(stringField(ratchet, "sealed")),
      };
    }),
  });
}

function parsePersistenceRestoreReport(
  value: unknown,
): BrowserPersistenceRestoreReport {
  const report = record(value, "persistence restore report");
  return {
    routes: nonNegativeInteger(numberField(report, "routes"), "restored routes"),
    destinationIdentities: nonNegativeInteger(
      numberField(report, "destinationIdentities"),
      "restored destination identities",
    ),
    tunnels: nonNegativeInteger(
      numberField(report, "tunnels"),
      "restored tunnels",
    ),
    ratchets: nonNegativeInteger(
      numberField(report, "ratchets"),
      "restored ratchets",
    ),
    refused: nonNegativeInteger(
      numberField(report, "refused"),
      "refused persistence records",
    ),
    dropped: nonNegativeInteger(
      numberField(report, "dropped"),
      "dropped persistence records",
    ),
  };
}

function requireWebUsb(): Available<BrowserUsb, "WebUSB"> {
  try {
    const usb = hostGlobal().navigator?.usb;
    return usb
      ? Tag("Available", usb)
      : Tag("HostApiUnavailable", { api: "WebUSB" });
  } catch {
    return Tag("HostApiUnavailable", { api: "WebUSB" });
  }
}

function requireWebBluetooth(): Available<BrowserBluetooth, "WebBluetooth"> {
  try {
    const bluetooth = hostGlobal().navigator?.bluetooth;
    return bluetooth
      ? Tag("Available", bluetooth)
      : Tag("HostApiUnavailable", { api: "WebBluetooth" });
  } catch {
    return Tag("HostApiUnavailable", { api: "WebBluetooth" });
  }
}

function requireBrowserWebSocket(): Available<typeof WebSocket, "WebSocket"> {
  try {
    const WebSocketCtor = hostGlobal().WebSocket;
    return WebSocketCtor
      ? Tag("Available", WebSocketCtor)
      : Tag("HostApiUnavailable", { api: "WebSocket" });
  } catch {
    return Tag("HostApiUnavailable", { api: "WebSocket" });
  }
}

async function openBrowserWebSocket(
  url: string,
  protocols?: string | readonly string[],
): Promise<WebSocketOpenOutcome> {
  const available = requireBrowserWebSocket();
  if (available.tag !== "Available") {
    return available;
  }
  const protocolList =
    protocols === undefined || typeof protocols === "string"
      ? protocols
      : [...protocols];
  let socket: WebSocket;
  try {
    const WebSocketCtor = available.data;
    socket =
      protocolList === undefined
        ? new WebSocketCtor(url)
        : new WebSocketCtor(url, protocolList);
  } catch (error) {
    return connectFailure("websocket", "TransportOpen", error);
  }
  try {
    socket.binaryType = "arraybuffer";
  } catch (error) {
    closeBrowserWebSocket(socket);
    return connectFailure("websocket", "TransportOpen", error);
  }
  return new Promise((resolve) => {
    let timeout: number | undefined;
    const cleanup = (): void => {
      if (timeout !== undefined) {
        globalThis.clearTimeout(timeout);
      }
      socket.removeEventListener("open", handleOpen);
      socket.removeEventListener("error", handleError);
      socket.removeEventListener("close", handleClose);
    };
    const handleOpen = (): void => {
      cleanup();
      resolve(Tag("Opened", socket));
    };
    const handleError = (): void => {
      cleanup();
      closeBrowserWebSocket(socket);
      resolve(
        Tag("ConnectionFailed", {
          interface: "websocket",
          stage: "TransportOpen",
          detail: `WebSocket connection failed for ${url}`,
        }),
      );
    };
    const handleClose = (): void => {
      cleanup();
      resolve(
        Tag("ConnectionFailed", {
          interface: "websocket",
          stage: "TransportOpen",
          detail: `WebSocket connection closed before opening for ${url}`,
        }),
      );
    };
    const handleTimeout = (): void => {
      cleanup();
      closeBrowserWebSocket(socket);
      resolve(
        Tag("TimedOut", {
          interface: "websocket",
          stage: "TransportOpen",
          timeoutMs: WEBSOCKET_CONNECT_TIMEOUT_MS,
        }),
      );
    };
    try {
      timeout = globalThis.setTimeout(handleTimeout, WEBSOCKET_CONNECT_TIMEOUT_MS);
      socket.addEventListener("open", handleOpen);
      socket.addEventListener("error", handleError);
      socket.addEventListener("close", handleClose);
    } catch (error) {
      cleanup();
      closeBrowserWebSocket(socket);
      resolve(connectFailure("websocket", "TransportOpen", error));
    }
  });
}

async function websocketMessageBytes(
  data: MessageEvent["data"],
  frameCap: number,
): Promise<WebSocketDecodeOutcome> {
  if (data instanceof ArrayBuffer) {
    return data.byteLength > frameCap
      ? frameTooLarge(data.byteLength, frameCap)
      : Tag("Decoded", new Uint8Array(data));
  }
  if (ArrayBuffer.isView(data)) {
    return data.byteLength > frameCap
      ? frameTooLarge(data.byteLength, frameCap)
      : Tag(
          "Decoded",
          new Uint8Array(data.buffer, data.byteOffset, data.byteLength),
        );
  }
  if (typeof Blob !== "undefined" && data instanceof Blob) {
    if (data.size > frameCap) {
      return frameTooLarge(data.size, frameCap);
    }
    try {
      return Tag("Decoded", new Uint8Array(await data.arrayBuffer()));
    } catch (error) {
      return Tag("TransferFailed", {
        direction: "Inbound",
        detail: describeHostError(error),
      });
    }
  }
  return Tag("UnsupportedFrame", {
    format: typeof data === "string" ? "Text" : "Unknown",
  });
}

function frameTooLarge(
  length: number,
  maximum: number,
): Extract<InterfaceSessionFailure, Tag<"FrameTooLarge", unknown>> {
  return Tag("FrameTooLarge", { length, maximum });
}

function closeBrowserWebSocket(
  socket: WebSocket | undefined,
): InterfaceCleanupFailure | undefined {
  try {
    if (
      socket &&
      (socket.readyState === WEBSOCKET_CONNECTING ||
        socket.readyState === WEBSOCKET_OPEN)
    ) {
      socket.close();
    }
  } catch (error) {
    return Tag("TransportCloseFailed", {
      detail: describeHostError(error),
    });
  }
  return undefined;
}

function firstUsbConfiguration(
  device: BrowserUsbDevice,
): UsbConfigurationOutcome {
  const configuration = device.configurations[0];
  if (!configuration) {
    return Tag("UnsupportedDevice", {
      interface: "usb-auto",
      capability: "USB configuration",
    });
  }
  return Tag("Configured", configuration);
}

type WebUsbEndpointPair = {
  interfaceNumber: number;
  alternate: BrowserUsbAlternateInterface;
  inEndpoint: BrowserUsbEndpoint;
  outEndpoint: BrowserUsbEndpoint;
};

function findWebUsbEndpointPair(
  configuration: BrowserUsbConfiguration,
): WebUsbEndpointPair | undefined {
  const vendorPairs: WebUsbEndpointPair[] = [];
  const bulkPairs: WebUsbEndpointPair[] = [];
  let fallbackPair: WebUsbEndpointPair | undefined;
  for (const iface of configuration.interfaces) {
    for (const alternate of iface.alternates) {
      const inEndpoint = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "in" && endpoint.type === "bulk",
      );
      const outEndpoint = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "out" && endpoint.type === "bulk",
      );
      if (inEndpoint && outEndpoint) {
        const pair = {
          interfaceNumber: iface.interfaceNumber,
          alternate,
          inEndpoint,
          outEndpoint,
        };
        if (alternate.interfaceClass === 0xff) {
          vendorPairs.push(pair);
        } else {
          bulkPairs.push(pair);
        }
        continue;
      }

      const fallbackIn = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "in",
      );
      const fallbackOut = alternate.endpoints.find(
        (endpoint) => endpoint.direction === "out",
      );
      if (!fallbackPair && fallbackIn && fallbackOut) {
        fallbackPair = {
          interfaceNumber: iface.interfaceNumber,
          alternate,
          inEndpoint: fallbackIn,
          outEndpoint: fallbackOut,
        };
      }
    }
  }
  return vendorPairs[0] ?? bulkPairs[0] ?? fallbackPair;
}

async function usbStage<T>(
  stage: InterfaceConnectStage,
  actionName: string,
  action: () => Promise<T>,
): Promise<UsbStageOutcome<T>> {
  try {
    return Tag("Completed", await action());
  } catch (error) {
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
      return Tag("PermissionDenied", {
        interface: "usb-auto",
        stage,
        detail: describeUsbError(error, actionName),
      });
    }
    if (name === "NotFoundError" && stage === "DeviceSelection") {
      return Tag("Cancelled", { interface: "usb-auto", stage });
    }
    return Tag("ConnectionFailed", {
      interface: "usb-auto",
      stage,
      detail: `USB ${actionName} failed: ${describeUsbError(error, actionName)}`,
    });
  }
}

async function bluetoothStage<T>(
  stage: InterfaceConnectStage,
  action: () => Promise<T>,
): Promise<BluetoothStageOutcome<T>> {
  try {
    return Tag("Completed", await action());
  } catch (error) {
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
      return Tag("PermissionDenied", {
        interface: "bluetooth",
        stage,
        detail: describeHostError(error),
      });
    }
    if (name === "NotFoundError" && stage === "DeviceSelection") {
      return Tag("Cancelled", { interface: "bluetooth", stage });
    }
    return Tag("ConnectionFailed", {
      interface: "bluetooth",
      stage,
      detail: describeHostError(error),
    });
  }
}

function describeUsbError(error: unknown, stage: string): string {
  const base = describeHostError(error);
  const name = domExceptionName(error);
  if (name === "SecurityError" || name === "NotAllowedError") {
    return `${base}. ${LINUX_WEBUSB_SETUP_HINT}`;
  }
  if (name === "NotFoundError" && stage.includes("request")) {
    return `${base}. No USB device was selected.`;
  }
  return base;
}

async function closeUsbDevice(
  device: BrowserUsbDevice,
): Promise<InterfaceCleanupFailure | undefined> {
  try {
    await device.close();
    return undefined;
  } catch (error) {
    return Tag("TransportCloseFailed", {
      detail: `close USB device: ${describeHostError(error)}`,
    });
  }
}

function disconnectBluetoothServer(
  server: BrowserBluetoothRemoteGattServer,
): InterfaceCleanupFailure | undefined {
  try {
    server.disconnect();
    return undefined;
  } catch (error) {
    return Tag("TransportCloseFailed", {
      detail: `disconnect Bluetooth server: ${describeHostError(error)}`,
    });
  }
}

function domExceptionName(error: unknown): string | undefined {
  return typeof DOMException !== "undefined" && error instanceof DOMException
    ? error.name
    : undefined;
}

function connectFailure<Name extends InterfaceName>(
  interfaceName: Name,
  stage: InterfaceConnectStage,
  error: unknown,
): ConnectionFailed<Name> | PermissionDenied<Name> | Cancelled<Name> {
  const name = domExceptionName(error);
  if (name === "SecurityError" || name === "NotAllowedError") {
    return Tag("PermissionDenied", {
      interface: interfaceName,
      stage,
      detail: describeHostError(error),
    });
  }
  if (name === "NotFoundError" && stage === "DeviceSelection") {
    return Tag("Cancelled", { interface: interfaceName, stage });
  }
  return Tag("ConnectionFailed", {
    interface: interfaceName,
    stage,
    detail: describeHostError(error),
  });
}

function describeHostError(error: unknown): string {
  if (typeof DOMException !== "undefined" && error instanceof DOMException) {
    return `${error.name}: ${error.message}`;
  }
  if (error instanceof Error) {
    return `${error.name}: ${error.message}`;
  }
  return String(error);
}

async function optionalBluetoothCharacteristic(
  service: BrowserBluetoothRemoteGattService,
  uuid: string,
): Promise<BrowserBluetoothRemoteGattCharacteristic | undefined> {
  try {
    return await service.getCharacteristic(uuid);
  } catch {
    return undefined;
  }
}

function characteristicBytes(
  event: BrowserBluetoothCharacteristicEvent,
): CharacteristicBytesOutcome {
  const value = event.target?.value;
  if (!value) {
    return Tag("ProtocolViolation", {
      protocol: "Bluetooth",
      detail: "Bluetooth characteristic event did not include a value",
    });
  }
  return Tag(
    "Decoded",
    new Uint8Array(value.buffer, value.byteOffset, value.byteLength),
  );
}

async function writeBluetoothValue(
  characteristic: BrowserBluetoothRemoteGattCharacteristic,
  bytes: Uint8Array,
): Promise<SessionWriteOutcome> {
  const value = arrayBufferForBluetooth(bytes);
  try {
    if (characteristic.writeValueWithoutResponse) {
      await characteristic.writeValueWithoutResponse(value);
    } else if (characteristic.writeValueWithResponse) {
      await characteristic.writeValueWithResponse(value);
    } else if (characteristic.writeValue) {
      await characteristic.writeValue(value);
    } else {
      return Tag("TransferFailed", {
        direction: "Outbound",
        detail: "Bluetooth characteristic does not support writes",
      });
    }
    return Tag("Written");
  } catch (error) {
    return Tag("TransferFailed", {
      direction: "Outbound",
      detail: describeHostError(error),
    });
  }
}

function arrayBufferForBluetooth(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function arrayBufferForUsb(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function browserUsbAutoChannelTag(device: BrowserUsbDevice): ChannelTag {
  const vendor = formatOptionalHex(device.vendorId);
  const product = formatOptionalHex(device.productId);
  const serial = device.serialNumber ?? "unknown";
  const nonce = nextBrowserUsbAutoTag;
  nextBrowserUsbAutoTag = (nextBrowserUsbAutoTag + 1) >>> 0;
  return channelTag(
    new TextEncoder().encode(`webusb:auto-usb:${vendor}:${product}:${serial}:${nonce}`),
  );
}

function canonicalWebSocketUrl(url: string | URL): CanonicalWebSocketOutcome {
  let target: URL;
  try {
    target = new URL(url.toString());
  } catch (error) {
    return Tag("InvalidTarget", {
      interface: "websocket",
      target: url.toString(),
      detail: describeHostError(error),
    });
  }
  if (target.protocol !== "ws:" && target.protocol !== "wss:") {
    return Tag("InvalidTarget", {
      interface: "websocket",
      target: target.toString(),
      detail: "WebSocket URL must use the ws or wss scheme",
    });
  }
  return Tag("Canonical", target.toString());
}

function runtimeRejected(
  operation: RuntimeOperation,
  error: unknown,
): RuntimeRejected {
  return Tag("RuntimeRejected", {
    operation,
    detail: describeHostError(error),
  });
}

function commandFailed(failure: CommandFailure): CommandSettlement {
  return Tag("Failed", failure);
}

function browserCommandFailure(
  operation: RuntimeOperation,
  error: unknown,
): CommandFailure {
  const detail = describeHostError(error);
  if (detail.includes("payload exceeds")) {
    return Tag("PayloadTooLarge");
  }
  return Tag("WriteFailed", { detail: `${operation}: ${detail}` });
}

function runtimeResponseTimeout(
  timeout: ResponseTimeout,
): { timeoutMillis?: number } {
  return match(timeout, {
    LinkDefault: () => ({}),
    Exact: ({ millis }) => ({
      timeoutMillis: nonNegativeInteger(millis, "timeoutMillis"),
    }),
  });
}

function runtimeResourceStrategy(
  strategy: ResourceStrategy,
): RuntimeResourceStrategy {
  return match(strategy, {
    Refuse: () => ({ strategy: "refuse" as const }),
    Accept: ({
      maximumUncompressedBytes,
      acceptCompressed,
    }) => ({
      strategy: "accept" as const,
      maximumUncompressedBytes: nonNegativeInteger(
        maximumUncompressedBytes,
        "maximumUncompressedBytes",
      ),
      acceptCompressed,
    }),
  });
}

function concatenateBytes(parts: readonly Uint8Array[]): Uint8Array {
  const length = parts.reduce(
    (total, part) => total + part.length,
    0,
  );
  const joined = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    joined.set(part, offset);
    offset += part.length;
  }
  return joined;
}

function fillEntropy(source: EntropySource, length: number): EntropyOutcome {
  let outcome: EntropyOutcome;
  try {
    outcome = source(length);
  } catch (error) {
    return Tag("EntropySourceFailed", { detail: describeHostError(error) });
  }
  if (outcome.tag !== "Filled") {
    return outcome;
  }
  if (outcome.data.length < length) {
    return Tag("InsufficientEntropy", {
      minimum: length,
      actual: outcome.data.length,
    });
  }
  return outcome;
}

function webCryptoIdentity(length: number): IdentityGenerationOutcome {
  try {
    if (!hostGlobal().crypto) {
      return Tag("HostApiUnavailable", { api: "Crypto" });
    }
    return Tag(
      "Generated",
      identitySecretKey(webCryptoBytes(length), length),
    );
  } catch (error) {
    return Tag("EntropySourceFailed", { detail: describeHostError(error) });
  }
}

async function loadOrCreateBleIdentity(
  store: StableIdentityStore,
): Promise<BleIdentityAvailability> {
  let loaded: StableIdentityLoadOutcome;
  try {
    loaded = await store.load(BLE_IDENTITY_LENGTH);
  } catch (error) {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `load Bluetooth LE identity: ${describeHostError(error)}`,
    });
  }
  if (loaded.tag === "Loaded") {
    const validated = bleIdentity(loaded.data);
    return validated.tag === "ValidBleIdentity"
      ? Tag("Available", validated.data)
      : Tag("StableIdentityUnavailable", {
          interface: "bluetooth",
          detail: `stored Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
        });
  }
  if (loaded.tag !== "Missing") {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: describeStableIdentityStoreFailure(loaded),
    });
  }
  let generatedBytes: Uint8Array;
  try {
    generatedBytes = webCryptoBytes(BLE_IDENTITY_LENGTH);
  } catch (error) {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `generate Bluetooth LE identity: ${describeHostError(error)}`,
    });
  }
  const validated = bleIdentity(generatedBytes);
  if (validated.tag !== "ValidBleIdentity") {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `generated Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
    });
  }
  const generated = validated.data;
  let saved: StableIdentitySaveOutcome;
  try {
    saved = await store.save(generated);
  } catch (error) {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: `save Bluetooth LE identity: ${describeHostError(error)}`,
    });
  }
  if (saved.tag !== "Saved") {
    return Tag("StableIdentityUnavailable", {
      interface: "bluetooth",
      detail: describeStableIdentityStoreFailure(saved),
    });
  }
  return Tag("Available", generated);
}

function describeStableIdentityStoreFailure(
  failure: StableIdentityStoreFailure,
): string {
  return match_into<string>().from(failure, {
    HostApiUnavailable: ({ api }) => `${api} is unavailable`,
    StableIdentityStoreFailed: ({ operation, detail }) =>
      `${operation} stable identity: ${detail}`,
    StoredStableIdentityInvalid: ({ detail }) => detail,
  });
}

function describePersistenceStoreFailure(
  failure: PersistenceStoreFailure,
): string {
  return match_into<string>().from(failure, {
    HostApiUnavailable: ({ api }) => `${api} is unavailable`,
    PersistenceStoreFailed: ({ operation, detail }) =>
      `${operation} persistence: ${detail}`,
    StoredPersistenceInvalid: ({ detail }) => detail,
  });
}

function unexpectedSessionFailure(error: unknown): Extract<
  InterfaceSessionFailure,
  Tag<"UnexpectedSessionFailure", unknown>
> {
  return Tag("UnexpectedSessionFailure", { detail: describeHostError(error) });
}

function closeFailed(
  causes: InterfaceCleanupFailures,
): Extract<InterfaceSessionFailure, Tag<"CloseFailed", unknown>> {
  return Tag("CloseFailed", { causes });
}

function hasCleanupFailures(
  causes: readonly InterfaceCleanupFailure[],
): causes is InterfaceCleanupFailures {
  return causes.length > 0;
}

function closedSessionOutcome(
  status: InterfaceSessionStatus,
): InterfaceCloseOutcome {
  return status.tag === "Failed" && status.data.tag === "CloseFailed"
    ? status.data
    : Tag("Closed");
}

function sessionFailureToConnectFailure(
  interfaceName: "bluetooth",
  stage: InterfaceConnectStage,
  failure: InterfaceSessionFailure,
): BluetoothConnectFailure {
  if (failure.tag === "RuntimeRejected") {
    return failure;
  }
  return Tag("ConnectionFailed", {
    interface: interfaceName,
    stage,
    detail: describeInterfaceSessionFailure(failure),
  });
}

function describeBluetoothConnectFailure(
  failure: BluetoothConnectFailure,
): string {
  return match(failure, {
    HostApiUnavailable: ({ api }) => `${api} is unavailable`,
    PermissionDenied: ({ detail }) => detail,
    Cancelled: ({ stage }) => `Bluetooth ${stage} was cancelled`,
    UnsupportedDevice: ({ capability }) =>
      `Bluetooth device does not provide ${capability}`,
    TimedOut: ({ stage, timeoutMs }) =>
      `Bluetooth ${stage} timed out after ${timeoutMs}ms`,
    ConnectionFailed: ({ detail }) => detail,
    AlreadyActive: ({ target }) => `${target} is already active`,
    StableIdentityUnavailable: ({ detail }) => detail,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

function webSocketCommandFailure(
  failure: Exclude<WebSocketConnectOutcome, Tag<"Connected", unknown>>,
): CommandFailure {
  return match_into<CommandFailure>().from(failure, {
    HostApiUnavailable: ({ api }) =>
      Tag("DeviceUnavailable", { detail: `${api} is unavailable` }),
    PermissionDenied: ({ detail }) => Tag("PermissionDenied", { detail }),
    Cancelled: ({ stage }) =>
      Tag("ConnectFailed", { detail: `WebSocket ${stage} was cancelled` }),
    AlreadyActive: ({ target }) =>
      Tag("BackendFailed", { detail: `${target} is already active` }),
    InvalidTarget: ({ detail }) => Tag("InvalidConfiguration", { detail }),
    TimedOut: ({ stage, timeoutMs }) =>
      Tag("ConnectFailed", {
        detail: `WebSocket ${stage} timed out after ${timeoutMs}ms`,
      }),
    ConnectionFailed: ({ detail }) => Tag("ConnectFailed", { detail }),
    RuntimeRejected: ({ operation, detail }) =>
      Tag("BackendFailed", { detail: `${operation}: ${detail}` }),
  });
}

function cooperativeBackendInfo(): BackendInfo {
  const webSocketAvailable = typeof globalThis.WebSocket === "function";
  const capabilities: CapabilityName[] = webSocketAvailable
    ? ["WebSocket", "BrowserRendezvous"]
    : [];
  const interfaceKinds: InterfaceKind[] = webSocketAvailable
    ? ["WebSocketClient", "BrowserRendezvous"]
    : [];
  return Object.freeze({
    backend: "Cooperative",
    capabilities: Object.freeze(capabilities),
    interfaceKinds: Object.freeze(interfaceKinds),
  });
}

function stableInterfaceKind(
  kind: RuntimeInterfaceKind,
): InterfaceKind | undefined {
  return ({
    "auto-usb-host": "AutomaticUsb",
    "auto-usb-device": "AutomaticUsb",
    rnode: "RNode",
    "bluetooth-auto": "AutomaticBluetoothLe",
    "bluetooth-peer": "AutomaticBluetoothLe",
    "auto-wifi": "BrowserRendezvous",
    "websocket-client": "WebSocketClient",
    "websocket-server": "WebSocketServer",
    "websocket-server-peer": "WebSocketServer",
    serial: "Serial",
    kiss: "Kiss",
    pipe: "Pipe",
  } satisfies Record<RuntimeInterfaceKind, InterfaceKind | undefined>)[kind];
}

function saturatingAdd(left: number, right: number): number {
  return Math.min(Number.MAX_SAFE_INTEGER, left + right);
}

function describeInterfaceSessionFailure(
  failure: InterfaceSessionFailure,
): string {
  return match_into<string>().from(failure, {
    Disconnected: ({ detail }) => detail,
    UnexpectedSessionFailure: ({ detail }) => detail,
    EntropySourceFailed: ({ detail }) => detail,
    TransferFailed: ({ direction, detail }) =>
      `${direction} transfer: ${detail}`,
    ProtocolViolation: ({ protocol, detail }) => `${protocol}: ${detail}`,
    UnsupportedFrame: ({ format }) =>
      `unsupported ${format.toLowerCase()} frame`,
    FrameTooLarge: ({ length, maximum }) =>
      `frame is ${length} bytes; maximum is ${maximum}`,
    OutboundQueueFull: ({ capacity }) =>
      `outbound queue reached ${capacity} frames`,
    CloseFailed: ({ causes }) =>
      causes.map((cause) => cause.data.detail).join("; "),
    HostApiUnavailable: ({ api }) => `${api} is unavailable`,
    InsufficientEntropy: ({ actual, minimum }) =>
      `entropy source returned ${actual} bytes; minimum is ${minimum}`,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

function normalizedWebSocketProtocols(
  protocols: string | readonly string[] | undefined,
): string | readonly string[] | undefined {
  if (protocols === undefined || typeof protocols === "string") {
    return protocols;
  }
  return protocols.length === 0 ? undefined : [...protocols];
}

function browserWebSocketChannelTag(
  url: string,
  protocols: string | readonly string[] | undefined,
): ChannelTag {
  const protocolList =
    protocols === undefined
      ? []
      : typeof protocols === "string"
        ? [protocols]
        : protocols;
  return channelTag(
    new TextEncoder().encode(JSON.stringify(["websocket-client", url, protocolList])),
  );
}

function byteKey(bytes: Uint8Array): string {
  let key = "";
  for (const byte of bytes) {
    key += byte.toString(16).padStart(2, "0");
  }
  return key;
}

function formatOptionalHex(value: number | undefined): string {
  return value === undefined ? "unknown" : value.toString(16).padStart(4, "0");
}

async function loadBundledWasm(): Promise<
  | Tag<"Loaded", PrnsWasmModule>
  | Tag<"WasmLoadFailed", { readonly detail: string }>
> {
  const moduleUrl = bundledWasmModuleUrl();
  try {
    const imported: unknown = await import(moduleUrl.href);
    const module = record(imported, "bundled WebAssembly module");
    const initialize = module.default;
    if (typeof initialize !== "function") {
      return Tag("WasmLoadFailed", {
        detail: "bundled WebAssembly module has no initializer",
      });
    }
    await initialize();
    return Tag("Loaded", imported as PrnsWasmModule);
  } catch (error) {
    return Tag("WasmLoadFailed", { detail: describeHostError(error) });
  }
}

function bundledWasmModuleUrl(): URL {
  return new URL("../../wasm/prns_wasm.js", import.meta.url);
}

function browserLimits(limits: HostLimits): HostLimits {
  return {
    pendingCommands: positiveInteger(
      limits.pendingCommands,
      "pending command limit",
    ),
    applicationEvents: positiveInteger(
      limits.applicationEvents,
      "application event limit",
    ),
    retainedEventBytes: positiveInteger(
      limits.retainedEventBytes,
      "retained event byte limit",
    ),
    diagnostics: positiveInteger(limits.diagnostics, "diagnostic limit"),
  };
}

function retainedBrowserEventBytes(event: PrnsApplicationEvent): number {
  return match_into<number>().from(event, {
    SingleDelivery: ({ plaintext }) => plaintext.length,
    Request: ({ data }) => data.length,
    Response: ({ data }) => data.length,
    ResponseSegment: ({ data }) => data.length,
    ResourceAvailable: ({ resource, metadata }) =>
      exactBytesAsSafeNumber(resource.totalBytes, "resource.totalBytes") +
      (metadata?.length ?? 0),
    ResourceSegment: ({ data, metadata }) =>
      data.length + (metadata?.length ?? 0),
    ResourceNeedsDecompression: ({ stream }) => stream.length,
    ChannelMessage: ({ data }) => data.length,
  });
}

function exactBytesAsSafeNumber(value: bigint, name: string): number {
  if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new PrnsValidationError(
      "invalid-number",
      `${name} exceeds the JavaScript safe-integer limit`,
    );
  }
  return Number(value);
}

function rawEventType(value: string): RawEventType {
  if (!RAW_EVENT_TYPES.has(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `runtime emitted event outside host contract: ${value}`,
    );
  }
  return value as RawEventType;
}

type RawLinkClosedReason = "timeout" | "peerClosed" | "malformedRtt";

const RAW_LINK_CLOSED_REASONS: ReadonlySet<string> =
  new Set<RawLinkClosedReason>([
    "timeout",
    "peerClosed",
    "malformedRtt",
  ]);

function linkClosedReason(
  value: string,
): "Timeout" | "PeerClosed" | "MalformedRtt" {
  if (!RAW_LINK_CLOSED_REASONS.has(value)) {
    throw new PrnsValidationError(
      "invalid-component",
      `unknown link close reason ${value}`,
    );
  }
  return match(value as RawLinkClosedReason, {
    timeout: () => "Timeout" as const,
    peerClosed: () => "PeerClosed" as const,
    malformedRtt: () => "MalformedRtt" as const,
  });
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function hostGlobal(): HostGlobal {
  return globalThis as HostGlobal;
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let i = 0; i < left.length; i += 1) {
    if (left[i] !== right[i]) {
      return false;
    }
  }
  return true;
}

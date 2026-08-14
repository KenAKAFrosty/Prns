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
  WebSocketFramingSelection,
} from "../contract.js";
import { MemoryResourceStream } from "../memory_resource.js";
import { AutoWifiInterface } from "./auto_wifi.js";
import { BluetoothInterface } from "./bluetooth.js";
import { byteKey } from "./bytes.js";
import {
  bigintField,
  bytesField,
  field,
  literalField,
  nonNegativeBigIntField,
  numberField,
  optionalArrayField,
  optionalBytesField,
  optionalNumber,
  record,
  stringField,
} from "./decoding.js";
import {
  connectFailure,
  describeHostError,
  domExceptionName,
} from "./host_errors.js";
import { hostGlobal } from "./host_apis.js";
import type {
  BrowserUsb,
  BrowserUsbAlternateInterface,
  BrowserUsbConfiguration,
  BrowserUsbDevice,
  BrowserUsbDeviceFilter,
  BrowserUsbEndpoint,
  HostApi,
  HostApiUnavailable,
} from "./host_apis.js";
import {
  BROWSER_PERSISTENCE_VERSION,
  BrowserLocalStorageBleIdentityStore,
  browserPersistenceStores,
  describePersistenceStoreFailure,
  describeStableIdentityStoreFailure,
  parseBrowserPersistedState,
  parsePersistenceRestoreReport,
} from "./persistence.js";
import type {
  BrowserPersistedState,
  BrowserPersistenceRestoreReport,
  BrowserPersistenceStore,
  IdentityLoadOutcome,
  IdentitySaveOutcome,
  IdentityStore,
  IdentityStoreFailure,
  PersistenceLoadOutcome,
  PersistenceStoreFailure,
  StableIdentityLoadOutcome,
  StableIdentitySaveOutcome,
  StableIdentityStore,
  StableIdentityStoreFailure,
} from "./persistence.js";
import {
  blobResourceSource,
  byteResourceSource,
  sendResourceFromSource,
} from "./resource_send.js";
import { browserResourceCompressor } from "./resource_compressor.js";
import {
  closeFailed,
  closedSessionOutcome,
  delay,
  describeInterfaceSessionFailure,
  hasCleanupFailures,
  unexpectedSessionFailure,
} from "./session.js";
import {
  BROWSER_RENDEZVOUS_FRAMING_SELECTION,
  WebSocketInterface,
} from "./websocket.js";
import type { WebSocketRuntimeRegistration } from "./websocket.js";
import type {
  ResourceSendSettlement,
  ResourceSource,
  RuntimeResourcePlanInput,
  RuntimeResourceSegmentInput,
  RuntimeResourceSegmentIssueInput,
} from "./resource_send.js";
import {
  BLE_IDENTITY_LENGTH,
  MIN_ENTROPY_BYTES,
  PrnsValidationError,
  appData,
  appName,
  aspect,
  bitrateBps,
  bleIdentity,
  channelTag,
  commandId,
  copyBytes,
  entropyBytes,
  hardwareMtu,
  hopCount,
  identitySecretKey,
  nonNegativeInteger,
  nowMillis,
  packetFrame,
  positiveInteger,
} from "./values.js";
import type {
  AppData,
  AppName,
  Aspect,
  BitrateBps,
  BleIdentity,
  BleIdentityValidationOutcome,
  ChannelTag,
  CommandId,
  EntropyBytes,
  HardwareMtu,
  HopCount,
  IdentitySecretKey,
  InstantMillis,
  PacketFrame,
} from "./values.js";

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
  WebSocketFramingSelection,
} from "../contract.js";
export {
  AutoWifiController,
  AutoWifiInterface,
  parseBrowserGatewayCatalog,
  validateBrowserGatewayUrl,
} from "./auto_wifi.js";
export { BluetoothInterface } from "./bluetooth.js";
export type {
  BluetoothConnectFailure,
  BluetoothConnectOutcome,
  BluetoothSession,
} from "./bluetooth.js";
export type { HostApi, HostApiUnavailable } from "./host_apis.js";
export {
  BROWSER_PERSISTENCE_VERSION,
  BrowserLocalStorageBleIdentityStore,
  BrowserLocalStorageIdentityStore,
  BrowserLocalStoragePersistenceStore,
} from "./persistence.js";
export type {
  BrowserPersistedRatchet,
  BrowserPersistedState,
  BrowserPersistenceStore,
  IdentityLoadFailure,
  IdentityLoadOutcome,
  IdentitySaveFailure,
  IdentitySaveOutcome,
  IdentityStore,
  IdentityStoreFailure,
  PersistenceLoadOutcome,
  PersistenceSaveOutcome,
  PersistenceStoreFailure,
  StableIdentityLoadOutcome,
  StableIdentitySaveOutcome,
  StableIdentityStore,
  StableIdentityStoreFailure,
} from "./persistence.js";
export { WebSocketInterface } from "./websocket.js";
export type {
  AutoWifiControllerCloseOutcome,
  AutoWifiControllerStatus,
  AutoWifiFailure,
  AutoWifiGatewayStatus,
  BrowserGatewayCatalogOutcome,
  BrowserRendezvousId,
} from "./auto_wifi.js";
export {
  BLE_IDENTITY_LENGTH,
  MIN_ENTROPY_BYTES,
  PrnsValidationError,
  appData,
  appName,
  aspect,
  bitrateBps,
  bleIdentity,
  channelTag,
  commandId,
  entropyBytes,
  hardwareMtu,
  hopCount,
  identitySecretKey,
  nowMillis,
  packetFrame,
} from "./values.js";
export type {
  AppData,
  AppName,
  Aspect,
  BitrateBps,
  BleIdentity,
  BleIdentityValidationOutcome,
  ChannelTag,
  CommandId,
  EntropyBytes,
  HardwareMtu,
  HopCount,
  IdentitySecretKey,
  InstantMillis,
  PacketFrame,
  PrnsValidationCode,
} from "./values.js";

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

export type StableIdentityUnavailable<
  Name extends InterfaceName = InterfaceName,
> = Tag<
  "StableIdentityUnavailable",
  { readonly interface: Name; readonly detail: string }
>;

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
  WebSocketFramingCodec: {
    new(selection: string): WebSocketFramingCodecBinding;
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

export type WebSocketFramingCodecBinding = {
  messageCap(): number;
  canReadOutbound(): boolean;
  rawFallbackIsArmed(): boolean;
  isDetecting(): boolean;
  rawFallbackDelayMillis(): number;
  decode(message: Uint8Array): WebSocketDecodeBatchBinding;
  stageOutbound(packet: PacketFrame): Uint8Array | undefined;
  resolveRawFallback(): Uint8Array | undefined;
};

export type WebSocketDecodeBatchBinding = {
  readonly packets: readonly Uint8Array[];
  readonly resolvedOutbound?: Uint8Array;
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

type UsbAutoInboundMessage =
  | Tag<"Hello">
  | Tag<"HelloAck", Uint8Array>
  | Tag<"Data", Uint8Array>;

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
type IdentityGenerationOutcome =
  | Tag<"Generated", IdentitySecretKey>
  | HostApiUnavailable<"Crypto">
  | Tag<"EntropySourceFailed", { readonly detail: string }>;
export type BleIdentityAvailability =
  | Tag<"Available", BleIdentity>
  | StableIdentityUnavailable<"bluetooth">;
type Available<Host, Api extends HostApi> =
  | Tag<"Available", Host>
  | HostApiUnavailable<Api>;
type UsbStageOutcome<Value> =
  | Tag<"Completed", Value>
  | PermissionDenied<"usb-auto">
  | Cancelled<"usb-auto">
  | ConnectionFailed<"usb-auto">;
type RuntimeOutboundDrainOutcome =
  | Tag<"Drained", readonly PrnsOutboundFrame[]>
  | RuntimeRejected;

const USB_AUTO_PROBE_INTERVAL_MS = 500;
const USB_AUTO_OUTBOUND_POLL_MS = 25;
const WEBUSB_MIN_TRANSFER_BYTES = 512;
const INTERFACE_OUTBOUND_QUEUE_DEPTH = 64;
let nextBrowserUsbAutoTag = 0;
const LINUX_WEBUSB_SETUP_HINT =
  "On Linux, run ./tools/prns device webusb install from the Prns repo root, " +
  "then unplug/replug the device and restart the browser. If this is Snap Chromium, " +
  "also run sudo snap connect chromium:raw-usb or use a non-Snap Chrome/Chromium build.";

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
  return browserPersistenceStores(root);
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

export type WebSocketSession = InterfaceSession & {
  readonly name: "websocket";
  readonly url: string;
  readonly framing: WebSocketFramingSelection;
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
  readonly framing?: WebSocketFramingSelection;
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
      WebSocketClient: ({ target, framing }) =>
        this.#attachWebSocket(target, "WebSocketClient", framing, routing),
      WebSocketServer: unsupported,
      BrowserRendezvous: ({ url }) =>
        this.#attachWebSocket(
          url,
          "BrowserRendezvous",
          BROWSER_RENDEZVOUS_FRAMING_SELECTION,
          routing,
        ),
    });
  }

  async #attachWebSocket(
    target: string,
    kind: InterfaceKind,
    framing: WebSocketFramingSelection,
    routing: InterfaceRoutingPolicy | undefined,
  ): Promise<CommandSettlement> {
    const connected = await this.interfaces.webSocket.connect(
      target,
      routing === undefined ? { framing } : { framing, routing },
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

  takeOutboundFor(
    interfaceId: InterfaceId,
    maximumFrames = Number.MAX_SAFE_INTEGER,
  ): OutboundTakeOutcome {
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
    const available = queued.concat(direct);
    const outbound = available.slice(0, maximumFrames);
    this.#outboundQueues.set(interfaceKey, available.slice(maximumFrames));
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

  createWebSocketFramingCodec(
    selection: WebSocketFramingSelection,
  ): WebSocketFramingCodecBinding {
    return new this.#wasm.WebSocketFramingCodec(
      wasmWebSocketFramingSelection(selection),
    );
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

  webSocketRegister(
    options: WebSocketRuntimeRegistration,
  ): InterfaceRegistrationOutcome<"websocket"> {
    try {
      return this.registerInterface({
        interfaceName: "websocket",
        kind: "websocket-client",
        channelTag: channelTag(options.channelTag),
        bitrateBps: options.bitrateBps,
        hardwareMtu: options.hardwareMtu,
        ...runtimeInterfaceRouting(options.routing),
      });
    } catch (error) {
      return runtimeRejected("register-interface", error);
    }
  }

  webSocketIngest(
    id: InterfaceId,
    bytes: Uint8Array,
  ): RuntimeIngestOutcome {
    try {
      return this.ingest(id, packetFrame(bytes));
    } catch (error) {
      return runtimeRejected("ingest", error);
    }
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

type RawUsbAutoMessageType = "hello" | "helloAck" | "data";

const RAW_USB_AUTO_MESSAGE_TYPES: ReadonlySet<string> =
  new Set<RawUsbAutoMessageType>(["hello", "helloAck", "data"]);

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

function wasmWebSocketFramingSelection(
  selection: WebSocketFramingSelection,
): string {
  switch (selection) {
    case "Auto":
      return "auto";
    case "RawPacket":
      return "raw";
    case "Hdlc":
      return "hdlc";
    case "Kiss":
      return "kiss";
  }
  const unreachable: never = selection;
  return unreachable;
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

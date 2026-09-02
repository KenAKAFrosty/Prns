import type { Tag } from "../casework.js";
import type {
  DestinationHash,
  IdentityHash,
  InterfaceId,
  InterfaceRoutingPolicy,
  LinkId,
  RequestHandlerConfig,
  RequestId,
  RequestPathHash,
} from "../contract.js";
import type { HostApiUnavailable } from "./host_apis.js";
import type { BrowserPersistedState } from "./persistence.js";
import type {
  RuntimeResourcePlanInput,
  RuntimeResourceSegmentInput,
} from "./resource_send.js";
import type {
  AppData,
  AppName,
  Aspect,
  BitrateBps,
  BleIdentity,
  ChannelTag,
  EntropyBytes,
  HardwareMtu,
  IdentitySecretKey,
  InstantMillis,
  PacketFrame,
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
  | "snapshot"
  | "projection-snapshot"
  | "worker-admission";

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
export type EntropySource = (length: number) => EntropyOutcome;

export type BleIdentityAvailability =
  | Tag<"Available", BleIdentity>
  | StableIdentityUnavailable<"bluetooth">;

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
  registerSingleDestination(
    options: RuntimeRegisterSingleDestinationOptions,
  ): DestinationHash;
  registerNodePage(options: RuntimeRegisterNodePageOptions): DestinationHash;
  announce(options: RuntimeAnnounceOptions): bigint;
  sendSinglePacket(options: RuntimeSendSinglePacketOptions): bigint;
  establishLink(options: RuntimeDestinationCommandOptions): bigint;
  requestPath(options: RuntimeDestinationCommandOptions): bigint;
  identify(options: RuntimeIdentifyOptions): bigint;
  sendLinkPacket(options: RuntimeLinkPayloadOptions): bigint;
  sendLinkPacketDirect?(
    linkId: LinkId,
    payload: Uint8Array,
    nowMs: InstantMillis,
    entropy: EntropyBytes,
  ): bigint;
  request(options: RuntimeRequestOptions): bigint;
  respond(options: RuntimeRespondOptions): bigint;
  resourceSegmentPlan(options: RuntimeResourcePlanInput): unknown;
  sendResourceSegment(options: RuntimeResourceSegmentInput): bigint;
  sendResourceSegmentWebCrypto?(options: RuntimeResourceSegmentInput): unknown;
  completeResourceSegmentSeal?(options: {
    readonly commandId: bigint;
    readonly linkId: LinkId;
    readonly streamNonce: Uint8Array;
    readonly noncePrefixedBytes: number;
    readonly sealed: Uint8Array;
    readonly salts: Uint8Array;
    readonly promotionEntropy: Uint8Array;
    readonly nowMs: InstantMillis;
  }): void;
  completeResourceSegmentSealDigests?(options: {
    readonly commandId: bigint;
    readonly linkId: LinkId;
    readonly streamNonce: Uint8Array;
    readonly noncePrefixedBytes: number;
    readonly sealed: Uint8Array;
    readonly salt: Uint8Array;
    readonly hash: Uint8Array;
    readonly proof: Uint8Array;
    readonly promotionEntropy: Uint8Array;
    readonly nowMs: InstantMillis;
  }): unknown;
  retryResourceSegmentSeal?(options: {
    readonly commandId: bigint;
    readonly linkId: LinkId;
    readonly streamNonce: Uint8Array;
    readonly noncePrefixedBytes: number;
    readonly nowMs: InstantMillis;
    readonly entropy: EntropyBytes;
  }): void;
  enableResourceWebCrypto?(): void;
  enableProtocolWebCrypto?(): void;
  enableProtocolCryptoOffload?(): void;
  takeProtocolCryptoJob?(): unknown | undefined;
  completeProtocolAnnounceValid?(
    id: number,
    nowMs: InstantMillis,
    entropy: EntropyBytes,
  ): void;
  completeProtocolAnnounceInvalid?(id: number): void;
  completeProtocolLinkProofValid?(
    id: number,
    sharedSecret: Uint8Array,
    nowMs: InstantMillis,
    entropy: EntropyBytes,
  ): void;
  completeProtocolLinkProofInvalid?(id: number): void;
  completeProtocolCryptoInline?(
    id: number,
    nowMs: InstantMillis,
    entropy: EntropyBytes,
  ): void;
  takeResourceOpenJob?(): unknown | undefined;
  completeResourceOpen?(options: {
    readonly linkId: LinkId;
    readonly hash: Uint8Array;
    readonly plaintext: Uint8Array;
    readonly nowMs: InstantMillis;
    readonly entropy: EntropyBytes;
  }): void;
  completeResourceOpenDigests?(options: {
    readonly linkId: LinkId;
    readonly hash: Uint8Array;
    readonly calculatedHash: Uint8Array;
    readonly proof: Uint8Array;
    readonly plaintext: Uint8Array;
    readonly nowMs: InstantMillis;
    readonly entropy: EntropyBytes;
  }): void;
  rejectResourceOpen?(options: {
    readonly linkId: LinkId;
    readonly hash: Uint8Array;
  }): void;
  retryResourceOpen?(options: {
    readonly linkId: LinkId;
    readonly hash: Uint8Array;
    readonly nowMs: InstantMillis;
    readonly entropy: EntropyBytes;
  }): void;
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
  ingestDirect?(
    interfaceId: InterfaceId,
    bytes: PacketFrame,
    nowMs: InstantMillis,
    entropy: EntropyBytes,
  ): void;
  drainEventBatch(): Uint8Array;
  drainCommandSettlementBatch?(): Uint8Array;
  drainEvents(): unknown[];
  drainOutboundBatch?(): Uint8Array;
  drainOutbound(): unknown[];
  persistedState(options: { readonly nowMs: InstantMillis }): unknown;
  restorePersistedState(
    options: BrowserPersistedState & { readonly nowMs: InstantMillis },
  ): unknown;
  snapshot(): unknown;
  snapshotPacked?(): Uint8Array;
  projectionSnapshot(request: RuntimeProjectionSnapshotRequest): unknown;
};

export type RuntimeProjectionSnapshotRequest = {
  readonly interfaces: boolean;
  readonly routes: boolean;
  readonly links: boolean;
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
  canStageMultipleOutbound(): boolean;
  canPassRawOutbound?(): boolean;
  canPassRawInbound?(): boolean;
  rawFallbackIsArmed(): boolean;
  isDetecting(): boolean;
  rawFallbackDelayMillis(): number;
  decode(message: Uint8Array): WebSocketDecodeBatchBinding;
  decodePacked?(message: Uint8Array): Uint8Array;
  stageOutbound(packet: PacketFrame): Uint8Array | undefined;
  releaseRawFallback(): Uint8Array | undefined;
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

export type RuntimeResourceStrategy =
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

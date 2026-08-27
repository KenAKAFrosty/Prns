import { Tag, match_into } from "./sdk/index.js";
import type {
  AutoWifiFailure,
  BluetoothConnectOutcome,
  CommandFailure,
  EntropyFailure,
  InterfaceCleanupFailure,
  InterfaceCloseOutcome,
  InterfaceSessionFailure,
  PrnsCreateOutcome,
  RuntimeRejected,
  Tag as Tagged,
  UsbAutoConnectOutcome,
  WebSocketConnectOutcome,
} from "./sdk/index.js";
import type { LxmfDeliveryProfileFailure } from "./lxmf.js";
import { boundedDetail } from "./presentation.js";

export type HostOperation =
  | "Create runtime"
  | "Connect WebSocket"
  | "Close WebSocket"
  | "Connect USB Auto"
  | "Close USB Auto"
  | "Connect Bluetooth"
  | "Close Bluetooth"
  | "Close Auto Wi-Fi";

export type HostOperationFailed = Tagged<
  "HostOperationFailed",
  { readonly operation: HostOperation; readonly detail: string }
>;

export type StartupFailure =
  | Tagged<"WasmLoadFailed", { readonly detail: string }>
  | LxmfDeliveryProfileFailure
  | HostOperationFailed
  | Exclude<PrnsCreateOutcome, Tagged<"Ready", unknown>>
  | RuntimeRejected;

export type UsbConnectFailure = Exclude<
  UsbAutoConnectOutcome,
  Tagged<"Connected", unknown>
>;

export type BluetoothConnectFailure = Exclude<
  BluetoothConnectOutcome,
  Tagged<"Connected", unknown>
>;

export type WebSocketConnectFailure = Exclude<
  WebSocketConnectOutcome,
  Tagged<"Connected", unknown>
>;

export type InterfaceCloseFailure = Exclude<
  InterfaceCloseOutcome,
  Tagged<"Closed", unknown>
>;

export function describeStartupFailure(outcome: StartupFailure): string {
  return match_into<string>().from(outcome, {
    WasmLoadFailed: ({ detail }) => `WebAssembly load: ${detail}`,
    LxmfDisplayNameTooLong: ({ actual, maximum }) =>
      `LXMF display name is ${actual} bytes; the maximum is ${maximum}`,
    HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
    ContractMismatch: ({
      actualAbi,
      actualProductVersion,
      requiredAbi,
      requiredProductVersion,
    }) =>
      `Host contract ${actualAbi}/${actualProductVersion} ` +
      `does not match ${requiredAbi}/${requiredProductVersion}`,
    HostApiUnavailable: ({ api }) =>
      `${api} is unavailable in this browser`,
    IdentityStoreFailed: ({ operation, detail }) =>
      `${operation} identity: ${detail}`,
    StoredIdentityInvalid: ({ detail }) => `Stored identity: ${detail}`,
    PersistenceStoreFailed: ({ operation, detail }) =>
      `${operation} persistence: ${detail}`,
    StoredPersistenceInvalid: ({ detail }) =>
      `Stored persistence: ${detail}`,
    EntropySourceFailed: ({ detail }) => detail,
    InsufficientEntropy: ({ actual, minimum }) =>
      `${actual} bytes received; ${minimum} required`,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function describeUsbConnectFailure(
  outcome: UsbConnectFailure | HostOperationFailed,
): string {
  return match_into<string>().from(outcome, {
    HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
    HostApiUnavailable: ({ api }) =>
      `${api} is unavailable in this browser`,
    PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
    Cancelled: ({ stage }) => `Cancelled during ${stage}`,
    AlreadyActive: ({ target }) => `Already active for ${target}`,
    UnsupportedDevice: ({ capability }) =>
      `Selected device lacks ${capability}`,
    ConnectionFailed: ({ stage, detail }) => `${stage}: ${detail}`,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function describeBluetoothConnectFailure(
  outcome: BluetoothConnectFailure | HostOperationFailed,
): string {
  return match_into<string>().from(outcome, {
    HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
    HostApiUnavailable: ({ api }) =>
      `${api} is unavailable in this browser`,
    PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
    Cancelled: ({ stage }) => `Cancelled during ${stage}`,
    UnsupportedDevice: ({ capability }) =>
      `Selected device lacks ${capability}`,
    TimedOut: ({ stage, timeoutMs }) =>
      `${stage} timed out after ${timeoutMs}ms`,
    ConnectionFailed: ({ stage, detail }) => `${stage}: ${detail}`,
    AlreadyActive: ({ target }) => `Already active for ${target}`,
    StableIdentityUnavailable: ({ detail }) => detail,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function describeWebSocketConnectFailure(
  outcome: WebSocketConnectFailure | HostOperationFailed,
): string {
  return match_into<string>().from(outcome, {
    HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
    HostApiUnavailable: ({ api }) => `${api} is unavailable in this browser`,
    PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
    Cancelled: ({ stage }) => `Cancelled during ${stage}`,
    AlreadyActive: ({ target }) => `Already active for ${target}`,
    InvalidTarget: ({ detail }) => detail,
    TimedOut: ({ stage, timeoutMs }) =>
      `${stage} timed out after ${timeoutMs}ms`,
    ConnectionFailed: ({ stage, detail }) => `${stage}: ${detail}`,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function describeInterfaceCloseFailure(
  outcome: InterfaceCloseFailure | HostOperationFailed,
): string {
  return match_into<string>().from(outcome, {
    HostOperationFailed: ({ operation, detail }) => `${operation}: ${detail}`,
    CloseFailed: ({ causes }) =>
      causes.map(describeCleanupFailure).join("; "),
  });
}

export function describeAutoWifiFailure(outcome: AutoWifiFailure): string {
  return match_into<string>().from(outcome, {
    HostApiUnavailable: ({ api }) =>
      `${api} is unavailable in this browser`,
    PermissionDenied: ({ stage, detail }) => `${stage}: ${detail}`,
    AlreadyActive: ({ target }) => `Already active for ${target}`,
    SelectionIdentityUnavailable: ({ detail }) => detail,
    DiscoveryFailed: ({ detail }) => detail,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function describeCommandFailure(outcome: CommandFailure): string {
  return match_into<string>().from(outcome, {
    NodeStopped: () => "The node is no longer running",
    Busy: () => "The pending command limit is full",
    PayloadTooLarge: () => "The payload exceeds the packet limit",
    ResponseTooLarge: () => "The response exceeds the requested limit",
    UnknownDestination: () => "The destination is not registered",
    NotSingleDestination: () => "The destination does not accept single packets",
    AnnounceAppDataTooLong: () => "The announce application data is too long",
    UnknownInterface: () => "The interface is not attached",
    NoRouteToDestination: () => "No route to the destination is known",
    NotDirectlyReachable: () => "The destination is not directly reachable",
    PacketCulled: () => "The packet was culled before delivery",
    DeliveryTimedOut: () => "Delivery timed out",
    InvalidBitrate: () => "The requested bitrate is invalid",
    BindFailed: ({ detail }) => `Bind failed: ${detail}`,
    WriteFailed: ({ detail }) => `Write failed: ${detail}`,
    UnsupportedByBackend: () => "The active backend does not support this command",
    UnknownLink: () => "The link does not exist",
    LinkNotActive: () => "The link is not active",
    EntropyUnavailable: () => "The browser entropy source is unavailable",
    NotLinkInitiator: () => "This node did not initiate the link",
    IdentityNotHeld: () => "The requested identity is not held by this node",
    UnknownRequestHandler: () => "The request handler is not registered",
    RequestPolicyNotAllowList: () => "The request handler does not use an allow list",
    RequestAllowListFull: () => "The request handler allow list is full",
    LinkBusy: () => "The link is busy",
    ResourceTableFull: () => "The resource table is full",
    ResourceMetadataTooLarge: () => "The resource metadata is too large",
    ResourceRejectedByPeer: () => "The peer rejected the resource",
    ResourceSequencingFailed: () => "The resource segment sequence failed",
    ResourcePredecessorFailed: () => "A preceding resource segment failed",
    ChannelWindowFull: () => "The channel send window is full",
    ChannelUntrackable: () => "The channel message cannot be tracked",
    InvalidChannelMessageType: () => "The channel message type is invalid",
    InvalidConfiguration: ({ detail }) => `Invalid configuration: ${detail}`,
    ResourceUploadCancelled: () => "The resource upload was cancelled",
    ResourceEarlyEof: () => "The resource upload ended before its declared length",
    ResourceLengthOverrun: () => "The resource upload exceeded its declared length",
    PermissionDenied: ({ detail }) => `Permission denied: ${detail}`,
    DeviceUnavailable: ({ detail }) => `Device unavailable: ${detail}`,
    ConnectFailed: ({ detail }) => `Connection failed: ${detail}`,
    BackendFailed: ({ detail }) => `Backend failed: ${detail}`,
  });
}

export function describeSessionFailure(
  outcome: InterfaceSessionFailure,
): string {
  return match_into<string>().from(outcome, {
    Disconnected: ({ detail }) => detail,
    TransferFailed: ({ direction, detail }) => `${direction}: ${detail}`,
    ProtocolViolation: ({ protocol, detail }) => `${protocol}: ${detail}`,
    UnsupportedFrame: ({ format }) => `${format} frame is unsupported`,
    FrameTooLarge: ({ length, maximum }) =>
      `${length} bytes exceeds the ${maximum}-byte limit`,
    OutboundQueueFull: ({ capacity }) =>
      `${capacity}-frame outbound queue is full`,
    CloseFailed: ({ causes }) =>
      causes.map(describeCleanupFailure).join("; "),
    UnexpectedSessionFailure: ({ detail }) => detail,
    HostApiUnavailable: ({ api }) =>
      `${api} is unavailable in this browser`,
    EntropySourceFailed: ({ detail }) => detail,
    InsufficientEntropy: ({ actual, minimum }) =>
      `${actual} bytes received; ${minimum} required`,
    RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
  });
}

export function describeEntropyFailure(outcome: EntropyFailure): string {
  return match_into<string>().from(outcome, {
    HostApiUnavailable: ({ api }) =>
      `${api} is unavailable in this browser`,
    EntropySourceFailed: ({ detail }) => detail,
    InsufficientEntropy: ({ actual, minimum }) =>
      `${actual} bytes received; ${minimum} required`,
  });
}

export function describeRuntimeRejected(outcome: RuntimeRejected): string {
  return `${outcome.data.operation}: ${outcome.data.detail}`;
}

export function hostOperationFailed(
  operation: HostOperation,
  error: unknown,
): HostOperationFailed {
  return Tag("HostOperationFailed", {
    operation,
    detail: describeHostError(error),
  });
}

export function describeHostOperationFailure(
  outcome: HostOperationFailed,
): string {
  return `${outcome.data.operation}: ${outcome.data.detail}`;
}

function describeCleanupFailure(outcome: InterfaceCleanupFailure): string {
  return match_into<string>().from(outcome, {
    RuntimeDetachFailed: ({ detail }) => `runtime detach: ${detail}`,
    TransportCloseFailed: ({ detail }) => `transport close: ${detail}`,
  });
}

export function describeHostError(error: unknown): string {
  if (error instanceof DOMException) {
    return boundedDetail(`${error.name}: ${error.message}`);
  }
  if (error instanceof Error) {
    return boundedDetail(`${error.name}: ${error.message}`);
  }
  if (typeof error === "string") {
    return boundedDetail(error);
  }
  return "The browser returned an opaque host failure";
}

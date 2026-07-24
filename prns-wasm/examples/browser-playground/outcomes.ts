import { Tag, match_into } from "./sdk/index.js";
import type {
  AutoWifiFailure,
  EntropyFailure,
  InterfaceCleanupFailure,
  InterfaceCloseOutcome,
  InterfaceSessionFailure,
  PrnsCreateOutcome,
  RuntimeRejected,
  Tag as Tagged,
  UsbAutoConnectOutcome,
} from "./sdk/index.js";
import type { LxmfDeliveryProfileFailure } from "./lxmf.js";
import { boundedDetail } from "./presentation.js";

export type HostOperation =
  | "Create runtime"
  | "Connect USB Auto"
  | "Close USB Auto"
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

export type UsbCloseFailure = Exclude<
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

export function describeUsbCloseFailure(
  outcome: UsbCloseFailure | HostOperationFailed,
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

import type { Tag } from "../casework.js";
import type { InterfaceId } from "../contract.js";
import type {
  EntropyFailure,
  InterfaceName,
  RuntimeRejected,
} from "./runtime_contract.js";

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

export type UnsupportedInterface<Name extends InterfaceName = InterfaceName> =
  Tag<
    "UnsupportedInterface",
    { readonly interface: Name; readonly host: "Browser" }
  >;

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
  | Extract<InterfaceSessionFailure, Tag<"CloseFailed", unknown>>
  | RuntimeRejected;

export type InterfaceSession = {
  readonly name: InterfaceName;
  readonly interfaceId: InterfaceId;
  readonly status: InterfaceSessionStatus;
  close(): Promise<InterfaceCloseOutcome>;
};

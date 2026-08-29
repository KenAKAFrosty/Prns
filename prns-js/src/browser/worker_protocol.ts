import { from } from "../casework.js";
import type { Tag } from "../casework.js";
import type {
  CommandSettlement,
  HostCommand,
  InterfaceId,
  PrnsLimits,
} from "../contract.js";
import type { WireBatch } from "../worker_wire/wire_batch.js";
import type {
  AutoWifiControllerCloseOutcome,
  AutoWifiControllerStatus,
} from "./auto_wifi/index.js";
import type { PrnsDiagnosticEvent } from "./events.js";
import type { InterfaceCloseOutcome, InterfaceSessionStatus } from "./interface_contract.js";
import type {
  BrowserPersistedState,
} from "./persistence.js";
import type {
  DestinationRegistrationOutcome,
  HostSnapshotOutcome,
  SendResourceOptions,
  SnapshotOutcome,
  StopOutcome,
} from "./index.js";
import type {
  BluetoothRuntimeHost,
} from "./bluetooth/runtime.js";
import type {
  RegisterSingleDestinationOptions,
} from "./runtime_contract.js";
import type {
  UsbAutoRuntimeHost,
} from "./usb_auto/runtime.js";
import type { BleIdentity, IdentitySecretKey } from "./values.js";
import type {
  WebSocketConnectOptions,
  WebSocketConnectOutcome,
  WebSocketSession,
} from "./websocket/index.js";

export const WORKER_WIRE_MAXIMUM_BYTES = 1024 * 1024;

export type WorkerInitialization = {
  readonly identity: IdentitySecretKey;
  readonly bleIdentity?: BleIdentity;
  readonly persistedState?: BrowserPersistedState;
  readonly persistenceEnabled: boolean;
  readonly limits: PrnsLimits;
  readonly resourceCompressionModuleUrl?: string;
  readonly wasmModuleUrl?: string;
  readonly autoWifiSelectionSeed?: Uint8Array;
};

export type WorkerSessionProjection = {
  readonly id: number;
  readonly name: "websocket";
  readonly interfaceId: InterfaceId;
  readonly status: InterfaceSessionStatus;
  readonly url: string;
  readonly framing: WebSocketSession["framing"];
};

export type WorkerCall =
  | Tag<"RegisterSingleDestination", RegisterSingleDestinationOptions>
  | Tag<"RegisterNodePage", Uint8Array>
  | Tag<"Execute", HostCommand>
  | Tag<
      "SendResourceBlob",
      {
        readonly linkId: Uint8Array;
        readonly blob: Blob;
        readonly options: SendResourceOptions;
      }
    >
  | Tag<"Snapshot">
  | Tag<"HostSnapshot">
  | Tag<"Stop">
  | Tag<
      "WebSocketConnect",
      { readonly url: string; readonly options: WebSocketConnectOptions }
    >
  | Tag<"AutoWifiStart">
  | Tag<"AutoWifiStatus">
  | Tag<"AutoWifiClose">
  | Tag<"InterfaceSessionClose", number>;

export type WorkerCallOutcomes = {
  readonly RegisterSingleDestination: DestinationRegistrationOutcome;
  readonly RegisterNodePage: DestinationRegistrationOutcome;
  readonly Execute: CommandSettlement;
  readonly SendResourceBlob: CommandSettlement;
  readonly Snapshot: SnapshotOutcome;
  readonly HostSnapshot: HostSnapshotOutcome;
  readonly Stop: {
    readonly stopOutcome: StopOutcome;
    readonly persistedState?: BrowserPersistedState;
    readonly snapshot: SnapshotOutcome;
    readonly hostSnapshot: HostSnapshotOutcome;
  };
  readonly WebSocketConnect:
    | Tag<"Connected", WorkerSessionProjection>
    | Exclude<WebSocketConnectOutcome, { readonly tag: "Connected" }>;
  readonly AutoWifiStart: AutoWifiControllerStatus;
  readonly AutoWifiStatus: AutoWifiControllerStatus;
  readonly AutoWifiClose: AutoWifiControllerCloseOutcome;
  readonly InterfaceSessionClose: InterfaceCloseOutcome;
};

export type WorkerCallOutcome<Call extends WorkerCall> =
  WorkerCallOutcomes[Call["tag"]];

export type WorkerInvocation = {
  readonly id: number;
  readonly call: WorkerCall;
};

export type WorkerSettlement = {
  readonly id: number;
  readonly call: WorkerCall["tag"];
  readonly outcome: unknown;
};

export type WorkerCapabilityCall =
  | Tag<
      "RegisterInterface",
      | Parameters<BluetoothRuntimeHost["registerInterface"]>[0]
      | Parameters<UsbAutoRuntimeHost["registerInterface"]>[0]
    >
  | Tag<"DeactivateInterface", Uint8Array>
  | Tag<
      "Ingest",
      { readonly interfaceId: Uint8Array; readonly bytes: Uint8Array }
    >
  | Tag<
      "TakeOutbound",
      { readonly interfaceId: Uint8Array; readonly maximumFrames?: number }
    >
  | Tag<"WaitForOutboundActivity", Uint8Array>
  | Tag<"CreateBluetoothReassembler">
  | Tag<
      "AbsorbBluetoothFragment",
      { readonly id: number; readonly bytes: Uint8Array }
    >
  | Tag<"ReleaseBluetoothReassembler", number>
  | Tag<"CreateUsbAutoDecoder">
  | Tag<
      "FeedUsbAutoDecoder",
      { readonly id: number; readonly bytes: Uint8Array }
    >
  | Tag<"ReleaseUsbAutoDecoder", number>;

export type WorkerCapabilityOutcomes = {
  readonly RegisterInterface:
    | Awaited<ReturnType<BluetoothRuntimeHost["registerInterface"]>>
    | Awaited<ReturnType<UsbAutoRuntimeHost["registerInterface"]>>;
  readonly DeactivateInterface: Awaited<ReturnType<BluetoothRuntimeHost["deactivateInterface"]>>;
  readonly Ingest: Awaited<ReturnType<BluetoothRuntimeHost["ingest"]>>;
  readonly TakeOutbound: Awaited<ReturnType<BluetoothRuntimeHost["takeOutboundFor"]>>;
  readonly WaitForOutboundActivity: Awaited<ReturnType<BluetoothRuntimeHost["waitForOutboundActivity"]>>;
  readonly CreateBluetoothReassembler: number;
  readonly AbsorbBluetoothFragment: Uint8Array | undefined;
  readonly ReleaseBluetoothReassembler: void;
  readonly CreateUsbAutoDecoder: number;
  readonly FeedUsbAutoDecoder: unknown[];
  readonly ReleaseUsbAutoDecoder: void;
};

export type WorkerCapabilityCallOutcome<Call extends WorkerCapabilityCall> =
  WorkerCapabilityOutcomes[Call["tag"]];

export type WorkerCapabilityInvocation = {
  readonly id: number;
  readonly call: WorkerCapabilityCall;
};

export type WorkerCapabilitySettlement = {
  readonly id: number;
  readonly call: WorkerCapabilityCall["tag"];
  readonly outcome: unknown;
};

export type WorkerControlRequest = Tag<"Calls", { readonly batch: WireBatch }>;
export type WorkerCapabilityRequest = Tag<"CapabilityCalls", { readonly batch: WireBatch }>;

export type WorkerControlResponse =
  | Tag<"Started", { readonly outcome: unknown }>
  | Tag<"Settlements", { readonly batch: WireBatch }>
  | Tag<
      "EventBackpressureExceeded",
      { readonly rejectedEventBytes: number }
    >
  | Tag<"ProtocolFailed", { readonly id?: number; readonly detail: string }>;

export type WorkerCapabilityResponse =
  | Tag<"CapabilitySettlements", { readonly batch: WireBatch }>
  | Tag<"ProtocolFailed", { readonly id?: number; readonly detail: string }>;

export type WorkerEventMessage =
  | Tag<"Batch", { readonly id: number; readonly buffer: ArrayBuffer }>
  | Tag<
      "Diagnostic",
      { readonly id: number; readonly event: PrnsDiagnosticEvent }
    >;

export type WorkerEventAcknowledgement = Tag<"Acknowledge", { readonly id: number }>;

export type WorkerStartMessage = Tag<
  "Initialize",
  {
    readonly initialization: WorkerInitialization;
    readonly control: MessagePort;
    readonly events: MessagePort;
    readonly capabilities: MessagePort;
  }
>;

export const { MakeTag: workerCall } = from<WorkerCall>();
export const { MakeTag: workerCapabilityCall } = from<WorkerCapabilityCall>();

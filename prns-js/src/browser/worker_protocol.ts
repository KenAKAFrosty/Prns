import { from } from "../casework.js";
import type { Tag } from "../casework.js";
import type {
  BackendInfo,
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
  ActiveLinkSnapshot,
  PrnsProjectionSnapshot,
  PrnsView,
  ProjectionRevision,
  ProjectionSynchronization,
} from "./projections.js";
import type {
  HostSnapshot,
  InterfaceSnapshot,
  LifecycleState,
  RouteSnapshot,
} from "../contract.js";
import type { PrnsDiagnosticEvent } from "./events.js";
import type {
  BluetoothRuntimeHost,
} from "./bluetooth/runtime.js";
import type {
  RegisterSingleDestinationOptions,
  RuntimeRejected,
} from "./runtime_contract.js";
import type {
  UsbAutoRuntimeHost,
} from "./usb_auto/runtime.js";
import type { BleIdentity, IdentitySecretKey } from "./values.js";
import type { CryptoExecution } from "./crypto_execution.js";
import type {
  WebSocketRuntimeRegistration,
  WebSocketConnectOptions,
  WebSocketConnectOutcome,
  WebSocketRuntimeHost,
  WebSocketSession,
} from "./websocket/index.js";

export const WORKER_WIRE_MAXIMUM_BYTES = 1024 * 1024;
export const MAXIMUM_PENDING_PROJECTION_SYNCHRONIZATIONS = 32;

export type WorkerInitialization = {
  readonly identity: IdentitySecretKey;
  readonly bleIdentity?: BleIdentity;
  readonly persistedState?: BrowserPersistedState;
  readonly persistenceEnabled: boolean;
  readonly limits: PrnsLimits;
  readonly resourceCompressionModuleUrl?: string;
  readonly crypto: CryptoExecution;
  readonly wasmModuleUrl?: string;
  readonly portableWasmModuleUrl?: string;
  readonly autoWifiSelectionSeed?: Uint8Array;
  readonly networkExecution: "EngineWorker" | "NetworkWorker";
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
  | Tag<
      "WebSocketConnect",
      { readonly url: string; readonly options: WebSocketConnectOptions }
    >
  | Tag<"AutoWifiStart">
  | Tag<"AutoWifiClose">
  | Tag<"InterfaceSessionClose", number>;

export type WorkerCallOutcomes = {
  readonly RegisterSingleDestination: DestinationRegistrationOutcome;
  readonly RegisterNodePage: DestinationRegistrationOutcome;
  readonly Execute: CommandSettlement;
  readonly SendResourceBlob: CommandSettlement;
  readonly Snapshot: SnapshotOutcome;
  readonly HostSnapshot: HostSnapshotOutcome;
  readonly WebSocketConnect:
    | Tag<"Connected", WorkerSessionProjection>
    | Exclude<WebSocketConnectOutcome, { readonly tag: "Connected" }>;
  readonly AutoWifiStart: AutoWifiControllerStatus;
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

export type WorkerSnapshotOutcome =
  | Tag<"PackedSnapshot", Uint8Array>
  | SnapshotOutcome;

export type WorkerCapabilityCall =
  | Tag<"RegisterWebSocket", WebSocketRuntimeRegistration>
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
      "NextOutbound",
      { readonly interfaceId: Uint8Array; readonly maximumFrames?: number }
    >
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
  readonly RegisterWebSocket: Awaited<ReturnType<WebSocketRuntimeHost["webSocketRegister"]>>;
  readonly RegisterInterface:
    | Awaited<ReturnType<BluetoothRuntimeHost["registerInterface"]>>
    | Awaited<ReturnType<UsbAutoRuntimeHost["registerInterface"]>>;
  readonly DeactivateInterface: Awaited<ReturnType<BluetoothRuntimeHost["deactivateInterface"]>>;
  readonly Ingest: Awaited<ReturnType<BluetoothRuntimeHost["ingest"]>>;
  readonly NextOutbound: Awaited<ReturnType<BluetoothRuntimeHost["nextOutboundFor"]>>;
  readonly CreateBluetoothReassembler: number;
  readonly AbsorbBluetoothFragment: Uint8Array | undefined;
  readonly ReleaseBluetoothReassembler: void;
  readonly CreateUsbAutoDecoder: number;
  readonly FeedUsbAutoDecoder: unknown[];
  readonly ReleaseUsbAutoDecoder: void;
};

export type WorkerCapabilityCallOutcome<Call extends WorkerCapabilityCall> =
  WorkerCapabilityOutcomes[Call["tag"]] | RuntimeRejected;

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
      "SessionStatusChanged",
      { readonly id: number; readonly status: InterfaceSessionStatus }
    >
  | Tag<"AutoWifiStatusChanged", AutoWifiControllerStatus>
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

export type WorkerEventRequest =
  | Tag<"Acknowledge", { readonly id: number }>
  | Tag<"ClaimApplicationEvents">
  | Tag<"ClaimDiagnostics">;

export type WorkerStartMessage = Tag<
  "Initialize",
  {
    readonly initialization: WorkerInitialization;
    readonly control: MessagePort;
    readonly events: MessagePort;
    readonly capabilities: MessagePort;
    readonly projections: MessagePort;
    readonly shutdown: MessagePort;
  }
>;

export type WorkerShutdownRequest = Tag<"Stop">;

export type WorkerShutdownState = {
  readonly stopOutcome: StopOutcome;
  readonly persistedState?: BrowserPersistedState;
  readonly snapshot: SnapshotOutcome;
  readonly hostSnapshot: HostSnapshotOutcome;
};

export type WorkerShutdownResponse =
  | Tag<"Stopped", WorkerShutdownState>
  | Tag<"ProtocolFailed", { readonly detail: string }>;

export type WorkerProjectionRequest =
  | Tag<"Observe", { readonly view: Exclude<PrnsView, Tag<"Diagnostics", unknown>> }>
  | Tag<"Unobserve", { readonly view: Exclude<PrnsView, Tag<"Diagnostics", unknown>> }>
  | Tag<"ObserveDiagnostics", { readonly maximumEvents: number }>
  | Tag<"Synchronize", { readonly id: number; readonly view: PrnsView }>
  | Tag<"AcknowledgeProjection", { readonly id: number }>;

export type WorkerProjectionUpdate =
  | Tag<"Lifecycle", PrnsProjectionSnapshot<LifecycleState>>
  | Tag<"Interfaces", PrnsProjectionSnapshot<readonly InterfaceSnapshot[]>>
  | Tag<"Routes", PrnsProjectionSnapshot<readonly RouteSnapshot[]>>
  | Tag<"Links", PrnsProjectionSnapshot<readonly ActiveLinkSnapshot[]>>
  | Tag<
      "DiagnosticsReset",
      PrnsProjectionSnapshot<readonly PrnsDiagnosticEvent[]>
    >
  | Tag<
      "DiagnosticsDelta",
      {
        readonly revision: ProjectionRevision;
        readonly dropped: number;
        readonly appended: readonly PrnsDiagnosticEvent[];
      }
    >;

export type WorkerProjectionMessage =
  | Tag<
      "ProjectionBatch",
      {
        readonly id: number;
        readonly batch: WireBatch;
      }
    >
  | Tag<
      "ProjectionSynchronized",
      {
        readonly id: number;
        readonly outcome: ProjectionSynchronization<unknown>;
      }
    >
  | Tag<"ProjectionProtocolFailed", { readonly detail: string }>;

export type WorkerReadyState = {
  readonly backendInfo: BackendInfo;
  readonly lifecycle: LifecycleState;
  readonly hostSnapshot: HostSnapshot;
};

export const { MakeTag: workerCall } = from<WorkerCall>();
export const { MakeTag: workerCapabilityCall } = from<WorkerCapabilityCall>();

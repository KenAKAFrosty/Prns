import { Tag } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import type {
  InterfaceId,
} from "../contract.js";
import { interfaceId } from "../contract.js";
import {
  prepareByteTransfer,
  receiveByteTransfer,
} from "./byte_transfer.js";
import type { TransferredByteBatch } from "./byte_transfer.js";
import type {
  InterfaceCloseOutcome,
  InterfaceSessionStatus,
} from "./interface_contract.js";
import type {
  PrnsOutboundFrame,
  TransferredInterfaceOutboundOutcome,
} from "./outbound.js";
import type {
  WebSocketConnectOptions,
} from "./websocket/index.js";
import type {
  WorkerCallOutcomes,
  WorkerCapabilityCall,
  WorkerCapabilityOutcomes,
} from "./worker_protocol.js";
import { packetFrameView } from "./values.js";

export type NetworkWorkerStartMessage = Tagged<
  "InitializeNetworkWorker",
  {
    readonly port: MessagePort;
    readonly wasmModuleUrl?: string;
  }
>;

export type EngineNetworkMessage =
  | Tagged<
      "Connect",
      {
        readonly id: number;
        readonly sessionId: number;
        readonly url: string;
        readonly options: WebSocketConnectOptions;
      }
    >
  | Tagged<"Close", { readonly id: number; readonly sessionId: number }>
  | Tagged<"HostSettlement", { readonly id: number; readonly outcome: unknown }>
  | Tagged<
      "IngressSettled",
      {
        readonly id: number;
        readonly count: number;
        readonly failures: readonly IngressFailure[];
      }
    >;

export type NetworkEngineMessage =
  | Tagged<"Ready">
  | Tagged<
      "ConnectSettled",
      {
        readonly id: number;
        readonly outcome: WorkerCallOutcomes["WebSocketConnect"];
      }
    >
  | Tagged<
      "CloseSettled",
      { readonly id: number; readonly outcome: InterfaceCloseOutcome }
    >
  | Tagged<
      "StatusChanged",
      { readonly sessionId: number; readonly status: InterfaceSessionStatus }
    >
  | Tagged<
      "HostCall",
      { readonly id: number; readonly call: WorkerCapabilityCall }
    >
  | Tagged<
      "IngressBatch",
      { readonly id: number; readonly batch: TransferredIngressBatch }
    >
  | Tagged<"ProtocolFailed", { readonly detail: string }>;

export type TransferredIngressItem = {
  readonly interfaceId: InterfaceId;
  readonly bytes: Uint8Array;
};

export type IngressFailure = {
  readonly index: number;
  readonly outcome: WorkerCapabilityOutcomes["Ingest"];
};

export type TransferredIngressBatch = {
  readonly interfaceIds: readonly InterfaceId[];
  readonly bytes: TransferredByteBatch;
};

export function receiveTransferredOutboundFrames(
  interfaceId: InterfaceId,
  outcome: Extract<
    TransferredInterfaceOutboundOutcome,
    Tagged<"TransferredOutbound", unknown>
  >,
): readonly PrnsOutboundFrame[] {
  return receiveByteTransfer(outcome.data).map((bytes) =>
    ({
      type: "frame",
      target: Tag("Interface", interfaceId),
      bytes: packetFrameView(bytes),
    }) satisfies PrnsOutboundFrame
  );
}

export function prepareIngressTransfer(
  items: readonly TransferredIngressItem[],
): TransferredIngressBatch {
  return {
    interfaceIds: items.map((item) => item.interfaceId),
    bytes: prepareByteTransfer(items.map((item) => item.bytes)),
  };
}

export function receiveIngressTransfer(
  batch: TransferredIngressBatch,
): readonly TransferredIngressItem[] {
  if (!Array.isArray(batch.interfaceIds)) {
    throw new TypeError("ingress transfer interfaces are malformed");
  }
  const bytes = receiveByteTransfer(batch.bytes);
  if (bytes.length !== batch.interfaceIds.length) {
    throw new TypeError("ingress transfer columns have different lengths");
  }
  return bytes.map((value, index) => {
    const rawInterfaceId = batch.interfaceIds[index];
    if (rawInterfaceId === undefined) {
      throw new TypeError("ingress transfer contains a missing interface");
    }
    return {
      interfaceId: interfaceId(rawInterfaceId),
      bytes: value,
    };
  });
}

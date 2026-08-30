import { Tag } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import type {
  InterfaceId,
} from "../contract.js";
import {
  INTERFACE_ID_LENGTH,
  interfaceId,
} from "../contract.js";
import type {
  InterfaceCloseOutcome,
  InterfaceSessionStatus,
} from "./interface_contract.js";
import type { PrnsOutboundFrame } from "./outbound.js";
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
        readonly failures: readonly PackedIngressFailure[];
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
  | Tagged<"IngressBatch", { readonly id: number; readonly buffer: ArrayBuffer }>
  | Tagged<"ProtocolFailed", { readonly detail: string }>;

export type PackedIngressItem = {
  readonly interfaceId: InterfaceId;
  readonly bytes: Uint8Array;
};

export type PackedIngressFailure = {
  readonly index: number;
  readonly outcome: WorkerCapabilityOutcomes["Ingest"];
};

export type PackedOutboundOutcome = Tagged<
  "PackedOutbound",
  { readonly buffer: ArrayBuffer }
>;

export function packOutboundFrames(
  frames: readonly PrnsOutboundFrame[],
): ArrayBuffer {
  let payloadBytes = 0;
  for (const frame of frames) {
    payloadBytes += frame.bytes.byteLength;
  }
  const headerBytes = 4 + frames.length * 4;
  const buffer = new ArrayBuffer(headerBytes + payloadBytes);
  const header = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  header.setUint32(0, frames.length, true);
  let payloadOffset = headerBytes;
  for (let index = 0; index < frames.length; index += 1) {
    const frame = frames[index];
    if (frame === undefined) {
      throw new TypeError("outbound frame batch contains a missing frame");
    }
    header.setUint32(4 + index * 4, frame.bytes.byteLength, true);
    bytes.set(frame.bytes, payloadOffset);
    payloadOffset += frame.bytes.byteLength;
  }
  return buffer;
}

export function unpackOutboundFrames(
  interfaceId: InterfaceId,
  buffer: ArrayBuffer,
): readonly PrnsOutboundFrame[] {
  if (buffer.byteLength < 4) {
    throw new TypeError("packed outbound batch is truncated");
  }
  const header = new DataView(buffer);
  const count = header.getUint32(0, true);
  const headerBytes = 4 + count * 4;
  if (headerBytes > buffer.byteLength) {
    throw new TypeError("packed outbound batch header is truncated");
  }
  const frames: PrnsOutboundFrame[] = new Array(count);
  let payloadOffset = headerBytes;
  for (let index = 0; index < count; index += 1) {
    const length = header.getUint32(4 + index * 4, true);
    if (payloadOffset + length > buffer.byteLength) {
      throw new TypeError("packed outbound batch payload is truncated");
    }
    frames[index] = {
      type: "frame",
      target: Tag("Interface", interfaceId),
      bytes: packetFrameView(new Uint8Array(buffer, payloadOffset, length)),
    };
    payloadOffset += length;
  }
  if (payloadOffset !== buffer.byteLength) {
    throw new TypeError("packed outbound batch has trailing bytes");
  }
  return frames;
}

export function packIngressItems(
  items: readonly PackedIngressItem[],
): ArrayBuffer {
  let payloadBytes = 0;
  for (const item of items) {
    payloadBytes += item.bytes.byteLength;
  }
  const rowBytes = INTERFACE_ID_LENGTH + 4;
  const headerBytes = 4 + items.length * rowBytes;
  const buffer = new ArrayBuffer(headerBytes + payloadBytes);
  const header = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  header.setUint32(0, items.length, true);
  let payloadOffset = headerBytes;
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    if (item === undefined) {
      throw new TypeError("ingress batch contains a missing item");
    }
    const rowOffset = 4 + index * rowBytes;
    bytes.set(item.interfaceId, rowOffset);
    header.setUint32(
      rowOffset + INTERFACE_ID_LENGTH,
      item.bytes.byteLength,
      true,
    );
    bytes.set(item.bytes, payloadOffset);
    payloadOffset += item.bytes.byteLength;
  }
  return buffer;
}

export function unpackIngressItems(
  buffer: ArrayBuffer,
): readonly PackedIngressItem[] {
  if (buffer.byteLength < 4) {
    throw new TypeError("packed ingress batch is truncated");
  }
  const header = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  const count = header.getUint32(0, true);
  const rowBytes = INTERFACE_ID_LENGTH + 4;
  const headerBytes = 4 + count * rowBytes;
  if (headerBytes > buffer.byteLength) {
    throw new TypeError("packed ingress batch header is truncated");
  }
  const items: PackedIngressItem[] = new Array(count);
  let payloadOffset = headerBytes;
  for (let index = 0; index < count; index += 1) {
    const rowOffset = 4 + index * rowBytes;
    const length = header.getUint32(
      rowOffset + INTERFACE_ID_LENGTH,
      true,
    );
    if (payloadOffset + length > buffer.byteLength) {
      throw new TypeError("packed ingress batch payload is truncated");
    }
    items[index] = {
      interfaceId: interfaceId(
        new Uint8Array(buffer, rowOffset, INTERFACE_ID_LENGTH),
      ),
      bytes: new Uint8Array(buffer, payloadOffset, length),
    };
    payloadOffset += length;
  }
  if (payloadOffset !== buffer.byteLength) {
    throw new TypeError("packed ingress batch has trailing bytes");
  }
  return items;
}

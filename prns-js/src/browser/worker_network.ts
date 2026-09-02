import { Tag, match_into } from "../casework.js";
import type {
  InterfaceId,
  WebSocketFramingSelection,
} from "../contract.js";
import {
  loadBundledWasm,
  loadWasmModule,
} from "./bootstrap.js";
import { describeHostError } from "./host_errors.js";
import type {
  InterfaceCloseOutcome,
  InterfaceSessionStatus,
} from "./interface_contract.js";
import type {
  InterfaceOutboundOutcome,
  TransferredInterfaceOutboundOutcome,
} from "./outbound.js";
import type {
  PrnsWasmModule,
  RuntimeRejected,
  WebSocketFramingCodecBinding,
} from "./runtime_contract.js";
import {
  WebSocketInterface,
} from "./websocket/index.js";
import type {
  WebSocketConnectOutcome,
  WebSocketRuntimeHost,
  WebSocketRuntimeRegistration,
  WebSocketSession,
} from "./websocket/index.js";
import type {
  EngineNetworkMessage,
  IngressFailure,
  NetworkEngineMessage,
  NetworkWorkerStartMessage,
  TransferredIngressItem,
} from "./worker_network_protocol.js";
import {
  prepareIngressTransfer,
  receiveTransferredOutboundFrames,
} from "./worker_network_protocol.js";
import {
  workerCapabilityCall,
} from "./worker_protocol.js";
import type {
  WorkerCapabilityCall,
  WorkerCapabilityCallOutcome,
  WorkerCapabilityOutcomes,
  WorkerSessionProjection,
} from "./worker_protocol.js";
import {
  bitrateBps,
  hardwareMtu,
  positiveInteger,
} from "./values.js";
import type {
  BitrateBps,
  HardwareMtu,
} from "./values.js";

type PendingHostCall = {
  readonly settle: (outcome: unknown) => void;
  readonly fail: (error: Error) => void;
};

type TrackedSession = {
  readonly session: WebSocketSession;
  readonly releaseStatus: () => void;
};

type PendingIngress = TransferredIngressItem & {
  readonly settle: (outcome: WorkerCapabilityOutcomes["Ingest"]) => void;
};

type InFlightIngress = {
  readonly id: number;
  readonly items: readonly PendingIngress[];
  readonly bytes: number;
};

type IngressFlowState =
  | Tag<"Idle">
  | Tag<"Scheduled">
  | Tag<"InFlight", InFlightIngress>
  | Tag<"Failed", { readonly detail: string }>;

const MAXIMUM_INGRESS_BATCH_ITEMS = 256;
const MAXIMUM_INGRESS_BATCH_BYTES = 1024 * 1024;
const MAXIMUM_OUTSTANDING_INGRESS_ITEMS = 4096;
const MAXIMUM_OUTSTANDING_INGRESS_BYTES = 4 * 1024 * 1024;

const workerScope = globalThis as typeof globalThis & {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<NetworkWorkerStartMessage>) => void,
  ): void;
};

workerScope.addEventListener("message", ({ data }) => {
  if (data?.tag === "InitializeNetworkWorker") {
    void initialize(data);
  }
});

async function initialize(message: NetworkWorkerStartMessage): Promise<void> {
  const port = message.data.port;
  port.start();
  try {
    const loaded = message.data.wasmModuleUrl === undefined
      ? await loadBundledWasm()
      : await loadWasmModule(new URL(message.data.wasmModuleUrl));
    if (loaded.tag !== "Loaded") {
      post(port, Tag("ProtocolFailed", { detail: loaded.data.detail }));
      return;
    }
    const host = new NetworkRuntimeHost(port, loaded.data);
    const sockets = new WebSocketInterface(host);
    const sessions = new Map<number, TrackedSession>();
    port.addEventListener(
      "message",
      (event: MessageEvent<EngineNetworkMessage>) => {
        try {
          match_into<void>().from(event.data, {
            Connect: ({ id, sessionId, url, options }) => {
              void connect(port, sockets, sessions, id, sessionId, url, options);
            },
            Close: ({ id, sessionId }) => {
              void closeSession(port, sessions, id, sessionId);
            },
            HostSettlement: ({ id, outcome }) => {
              host.settle(id, outcome);
            },
            IngressSettled: ({ id, count, failures }) => {
              host.settleIngress(id, count, failures);
            },
          });
        } catch (error) {
          const detail = describeHostError(error);
          host.fail(detail);
          post(port, Tag("ProtocolFailed", { detail }));
        }
      },
    );
    post(port, Tag("Ready"));
  } catch (error) {
    post(port, Tag("ProtocolFailed", { detail: describeHostError(error) }));
  }
}

async function connect(
  port: MessagePort,
  sockets: WebSocketInterface,
  sessions: Map<number, TrackedSession>,
  id: number,
  sessionId: number,
  url: string,
  options: Parameters<WebSocketInterface["connect"]>[1],
): Promise<void> {
  try {
    if (sessions.has(sessionId)) {
      throw new Error(`network worker already tracks session ${sessionId}`);
    }
    const outcome = await sockets.connect(url, options);
    if (outcome.tag !== "Connected") {
      post(port, Tag("ConnectSettled", { id, outcome }));
      return;
    }
    const session = outcome.data;
    const releaseStatus = session.subscribeStatus((status) => {
      post(port, Tag("StatusChanged", { sessionId, status }));
      if (status.tag === "Closed" || status.tag === "Failed") {
        releaseStatus();
        sessions.delete(sessionId);
      }
    });
    sessions.set(sessionId, { session, releaseStatus });
    const projection: WorkerSessionProjection = {
      id: sessionId,
      name: "websocket",
      interfaceId: session.interfaceId,
      status: session.status,
      url: session.url,
      framing: session.framing,
    };
    post(port, Tag("ConnectSettled", {
      id,
      outcome: Tag("Connected", projection),
    }));
  } catch (error) {
    post(port, Tag("ProtocolFailed", { detail: describeHostError(error) }));
  }
}

async function closeSession(
  port: MessagePort,
  sessions: Map<number, TrackedSession>,
  id: number,
  sessionId: number,
): Promise<void> {
  try {
    const tracked = sessions.get(sessionId);
    const outcome = tracked === undefined
      ? Tag("Closed")
      : await tracked.session.close();
    if (sessions.get(sessionId) === tracked) {
      tracked?.releaseStatus();
      sessions.delete(sessionId);
    }
    post(port, Tag("CloseSettled", { id, outcome }));
  } catch (error) {
    post(port, Tag("ProtocolFailed", { detail: describeHostError(error) }));
  }
}

class NetworkRuntimeHost implements WebSocketRuntimeHost {
  readonly #port: MessagePort;
  readonly #wasm: PrnsWasmModule;
  readonly #pending = new Map<number, PendingHostCall>();
  readonly #pendingIngress: PendingIngress[] = [];
  #pendingIngressBytes = 0;
  #ingressState: IngressFlowState = Tag("Idle");
  #nextId = 1;
  #nextIngressId = 1;

  constructor(port: MessagePort, wasm: PrnsWasmModule) {
    this.#port = port;
    this.#wasm = wasm;
  }

  runtimeReadiness(): Tag<"Ready"> {
    return Tag("Ready");
  }

  webSocketRegister(
    options: WebSocketRuntimeRegistration,
  ): Promise<Awaited<ReturnType<WebSocketRuntimeHost["webSocketRegister"]>>> {
    return this.#call(workerCapabilityCall("RegisterWebSocket", options));
  }

  deactivateInterface(
    interfaceId: InterfaceId,
  ): Promise<Awaited<ReturnType<WebSocketRuntimeHost["deactivateInterface"]>>> {
    return this.#call(workerCapabilityCall("DeactivateInterface", interfaceId));
  }

  webSocketIngest(
    interfaceId: InterfaceId,
    bytes: Uint8Array,
  ): Promise<Awaited<ReturnType<WebSocketRuntimeHost["webSocketIngest"]>>> {
    if (this.#ingressState.tag === "Failed") {
      return Promise.resolve(
        networkIngressFailed(this.#ingressState.data.detail),
      );
    }
    const inFlight = this.#ingressState.tag === "InFlight"
      ? this.#ingressState.data
      : undefined;
    if (
      this.#pendingIngress.length + (inFlight?.items.length ?? 0) >=
        MAXIMUM_OUTSTANDING_INGRESS_ITEMS ||
      this.#pendingIngressBytes + (inFlight?.bytes ?? 0) + bytes.byteLength >
        MAXIMUM_OUTSTANDING_INGRESS_BYTES
    ) {
      return Promise.resolve(networkIngressBusy());
    }
    return new Promise((settle) => {
      this.#pendingIngress.push({ interfaceId, bytes, settle });
      this.#pendingIngressBytes += bytes.byteLength;
      this.#scheduleIngress();
    });
  }

  async nextOutboundFor(
    interfaceId: InterfaceId,
    maximumFrames?: number,
  ): Promise<InterfaceOutboundOutcome> {
    const outcome = await this.#call(workerCapabilityCall(
      "NextOutbound",
      maximumFrames === undefined
        ? { interfaceId }
        : { interfaceId, maximumFrames },
    )) as InterfaceOutboundOutcome | TransferredInterfaceOutboundOutcome;
    if (outcome.tag === "TransferredOutbound") {
      return Tag(
        "Outbound",
        receiveTransferredOutboundFrames(interfaceId, outcome) as Extract<
          InterfaceOutboundOutcome,
          { readonly tag: "Outbound" }
        >["data"],
      );
    }
    return outcome as InterfaceOutboundOutcome;
  }

  createWebSocketFramingCodec(
    selection: WebSocketFramingSelection,
  ): WebSocketFramingCodecBinding {
    return new this.#wasm.WebSocketFramingCodec(wasmFramingSelection(selection));
  }

  websocketBitrateBps(): BitrateBps {
    return bitrateBps(this.#wasm.websocketBitrateBps());
  }

  websocketHardwareMtu(): HardwareMtu {
    return hardwareMtu(this.#wasm.websocketHardwareMtu());
  }

  websocketFrameCap(): number {
    return positiveInteger(this.#wasm.websocketFrameCap(), "WebSocket frame cap");
  }

  settle(id: number, outcome: unknown): void {
    const pending = this.#pending.get(id);
    if (pending === undefined) {
      throw new Error(`network worker received unknown host settlement ${id}`);
    }
    this.#pending.delete(id);
    pending.settle(outcome);
  }

  settleIngress(
    id: number,
    count: number,
    failures: readonly IngressFailure[],
  ): void {
    if (this.#ingressState.tag === "Failed") {
      return;
    }
    if (
      this.#ingressState.tag !== "InFlight" ||
      this.#ingressState.data.id !== id
    ) {
      throw new Error(`network worker received unknown ingress settlement ${id}`);
    }
    const inFlight = this.#ingressState.data;
    if (count !== inFlight.items.length) {
      throw new Error(
        `network worker ingress settlement ${id} has count ${count}, expected ${inFlight.items.length}`,
      );
    }
    const outcomes = new Map<number, WorkerCapabilityOutcomes["Ingest"]>();
    for (const failure of failures) {
      if (
        !Number.isSafeInteger(failure.index) ||
        failure.index < 0 ||
        failure.index >= count ||
        failure.outcome.tag === "Accepted" ||
        outcomes.has(failure.index)
      ) {
        throw new Error(
          `network worker ingress settlement ${id} has an invalid failure`,
        );
      }
      outcomes.set(failure.index, failure.outcome);
    }
    this.#ingressState = Tag("Idle");
    for (let index = 0; index < inFlight.items.length; index += 1) {
      const item = inFlight.items[index];
      if (item === undefined) {
        throw new Error(`network worker ingress settlement ${id} is sparse`);
      }
      item.settle(outcomes.get(index) ?? Tag("Accepted"));
    }
    this.#scheduleIngress();
  }

  #scheduleIngress(): void {
    if (
      this.#ingressState.tag !== "Idle" ||
      this.#pendingIngress.length === 0
    ) {
      return;
    }
    this.#ingressState = Tag("Scheduled");
    queueMicrotask(() => {
      try {
        this.#flushIngress();
      } catch (error) {
        const detail = describeHostError(error);
        this.fail(detail);
        post(this.#port, Tag("ProtocolFailed", { detail }));
      }
    });
  }

  #flushIngress(): void {
    if (this.#ingressState.tag !== "Scheduled") {
      throw new Error("network worker ingress flush was not scheduled");
    }
    if (this.#pendingIngress.length === 0) {
      this.#ingressState = Tag("Idle");
      return;
    }
    let count = 0;
    let bytes = 0;
    while (
      count < this.#pendingIngress.length &&
      count < MAXIMUM_INGRESS_BATCH_ITEMS
    ) {
      const item = this.#pendingIngress[count];
      if (item === undefined) {
        throw new Error("network worker ingress queue is sparse");
      }
      if (
        count > 0 &&
        bytes + item.bytes.byteLength > MAXIMUM_INGRESS_BATCH_BYTES
      ) {
        break;
      }
      count += 1;
      bytes += item.bytes.byteLength;
    }
    const items = this.#pendingIngress.splice(0, count);
    this.#pendingIngressBytes -= bytes;
    const id = this.#nextIngressId;
    this.#nextIngressId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    this.#ingressState = Tag("InFlight", { id, items, bytes });
    const batch = prepareIngressTransfer(items);
    post(
      this.#port,
      Tag("IngressBatch", { id, batch }),
      batch.bytes.buffers,
    );
  }

  fail(detail: string): void {
    if (this.#ingressState.tag === "Failed") {
      return;
    }
    const inFlight = this.#ingressState.tag === "InFlight"
      ? this.#ingressState.data.items
      : [];
    const ingress = [...inFlight, ...this.#pendingIngress];
    this.#pendingIngress.length = 0;
    this.#pendingIngressBytes = 0;
    this.#ingressState = Tag("Failed", { detail });
    const outcome = networkIngressFailed(detail);
    for (const item of ingress) {
      item.settle(outcome);
    }
    const error = new Error(detail);
    for (const pending of this.#pending.values()) {
      pending.fail(error);
    }
    this.#pending.clear();
  }

  #call<Call extends WorkerCapabilityCall>(
    call: Call,
    transfer: readonly Transferable[] = [],
  ): Promise<WorkerCapabilityCallOutcome<Call>> {
    if (this.#ingressState.tag === "Failed") {
      return Promise.reject(new Error(this.#ingressState.data.detail));
    }
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    return new Promise((settle, fail) => {
      this.#pending.set(id, {
        settle: settle as (outcome: unknown) => void,
        fail,
      });
      const message: NetworkEngineMessage = Tag("HostCall", { id, call });
      try {
        this.#port.postMessage(message, [...transfer]);
      } catch (error) {
        this.#pending.delete(id);
        fail(error);
      }
    });
  }
}

function networkIngressBusy(): RuntimeRejected {
  return Tag("RuntimeRejected", {
    operation: "worker-admission",
    detail: "network Worker ingress channel is busy",
  });
}

function networkIngressFailed(detail: string): RuntimeRejected {
  return Tag("RuntimeRejected", {
    operation: "ingest",
    detail,
  });
}

function wasmFramingSelection(selection: WebSocketFramingSelection): string {
  return match_into<string>().from(Tag(selection), {
    Auto: () => "auto",
    RawPacket: () => "raw",
    Hdlc: () => "hdlc",
    Kiss: () => "kiss",
  });
}

function post(
  port: MessagePort,
  message: NetworkEngineMessage,
  transfer: readonly Transferable[] = [],
): void {
  port.postMessage(message, [...transfer]);
}

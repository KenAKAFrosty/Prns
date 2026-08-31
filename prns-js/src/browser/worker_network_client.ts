import { Tag, match_into } from "../casework.js";
import type {
  InterfaceSessionStatus,
} from "./interface_contract.js";
import { describeHostError } from "./host_errors.js";
import { dispatchWorkerCapability } from "./worker_engine_bridge.js";
import type { Prns } from "./index.js";
import {
  packOutboundFrames,
  unpackIngressItems,
} from "./worker_network_protocol.js";
import type {
  EngineNetworkMessage,
  NetworkEngineMessage,
  NetworkWorkerStartMessage,
  PackedIngressFailure,
} from "./worker_network_protocol.js";
import type {
  WebSocketConnectOptions,
} from "./websocket/index.js";
import type {
  WorkerCallOutcomes,
  WorkerCapabilityCall,
  WorkerInitialization,
} from "./worker_protocol.js";
import { workerCapabilityCall } from "./worker_protocol.js";

type PendingNetworkCall = {
  readonly operation: "Connect" | "Close";
  readonly settle: (outcome: unknown) => void;
  readonly fail: (error: Error) => void;
};

type NetworkClientStartOutcome =
  | Tag<"Ready", WorkerNetworkClient>
  | Tag<"Failed", { readonly detail: string }>;

export class WorkerNetworkClient {
  readonly #worker: Worker;
  readonly #port: MessagePort;
  readonly #engine: Prns;
  readonly #statusChanged: (
    sessionId: number,
    status: InterfaceSessionStatus,
  ) => void;
  readonly #networkFailed: (detail: string) => void;
  readonly #pending = new Map<number, PendingNetworkCall>();
  #nextId = 1;
  #startSettled = false;
  #startResolve: ((outcome: NetworkClientStartOutcome) => void) | undefined;
  #failed = false;

  static async create(
    initialization: WorkerInitialization,
    engine: Prns,
    statusChanged: (
      sessionId: number,
      status: InterfaceSessionStatus,
    ) => void,
    networkFailed: (detail: string) => void,
  ): Promise<NetworkClientStartOutcome> {
    let worker: Worker;
    try {
      worker = new Worker(new URL("./worker_network.js", import.meta.url), {
        type: "module",
        name: "prns-network",
      });
    } catch (error) {
      return Tag("Failed", { detail: describeHostError(error) });
    }
    const channel = new MessageChannel();
    const client = new WorkerNetworkClient(
      worker,
      channel.port1,
      engine,
      statusChanged,
      networkFailed,
    );
    const started = client.started();
    const message: NetworkWorkerStartMessage = Tag("InitializeNetworkWorker", {
      port: channel.port2,
      ...(initialization.wasmModuleUrl === undefined
        ? {}
        : { wasmModuleUrl: initialization.wasmModuleUrl }),
    });
    worker.postMessage(message, [channel.port2]);
    return started;
  }

  private constructor(
    worker: Worker,
    port: MessagePort,
    engine: Prns,
    statusChanged: (
      sessionId: number,
      status: InterfaceSessionStatus,
    ) => void,
    networkFailed: (detail: string) => void,
  ) {
    this.#worker = worker;
    this.#port = port;
    this.#engine = engine;
    this.#statusChanged = statusChanged;
    this.#networkFailed = networkFailed;
    port.addEventListener(
      "message",
      (event: MessageEvent<NetworkEngineMessage>) => {
        this.#receive(event.data);
      },
    );
    port.start();
    worker.addEventListener("error", (event) => {
      this.#fail(event.message || "network Worker failed");
    });
    worker.addEventListener("messageerror", () => {
      this.#fail("network Worker message could not be decoded");
    });
  }

  started(): Promise<NetworkClientStartOutcome> {
    return new Promise((resolve) => {
      this.#startResolve = resolve;
    });
  }

  connect(
    sessionId: number,
    url: string,
    options: WebSocketConnectOptions,
  ): Promise<WorkerCallOutcomes["WebSocketConnect"]> {
    return this.#call(
      "Connect",
      (id) => Tag("Connect", { id, sessionId, url, options }),
    );
  }

  closeSession(
    sessionId: number,
  ): Promise<WorkerCallOutcomes["InterfaceSessionClose"]> {
    return this.#call(
      "Close",
      (id) => Tag("Close", { id, sessionId }),
    );
  }

  terminate(): void {
    this.#failed = true;
    this.#port.close();
    this.#worker.terminate();
    this.#failPending("network Worker terminated");
  }

  #call<Outcome>(
    operation: PendingNetworkCall["operation"],
    request: (id: number) => EngineNetworkMessage,
  ): Promise<Outcome> {
    if (this.#failed) {
      return Promise.reject(new Error("network Worker is unavailable"));
    }
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    return new Promise((settle, fail) => {
      this.#pending.set(id, {
        operation,
        settle: settle as (outcome: unknown) => void,
        fail,
      });
      this.#port.postMessage(request(id));
    });
  }

  #receive(message: NetworkEngineMessage): void {
    try {
      match_into<void>().from(message, {
        Ready: () => {
          if (this.#startSettled || this.#startResolve === undefined) {
            throw new Error("network Worker sent duplicate readiness");
          }
          this.#startSettled = true;
          this.#startResolve(Tag("Ready", this));
          this.#startResolve = undefined;
        },
        ConnectSettled: ({ id, outcome }) => {
          this.#settle(id, "Connect", outcome);
        },
        CloseSettled: ({ id, outcome }) => {
          this.#settle(id, "Close", outcome);
        },
        StatusChanged: ({ sessionId, status }) => {
          this.#statusChanged(sessionId, status);
        },
        HostCall: ({ id, call }) => {
          void this.#performHostCall(id, call);
        },
        IngressBatch: ({ id, buffer }) => {
          void this.#performIngressBatch(id, buffer);
        },
        ProtocolFailed: ({ detail }) => {
          this.#fail(detail);
        },
      });
    } catch (error) {
      this.#fail(describeHostError(error));
    }
  }

  async #performHostCall(
    id: number,
    call: WorkerCapabilityCall,
  ): Promise<void> {
    try {
      if (call.tag === "NextOutbound") {
        const outcome = await dispatchWorkerCapability(this.#engine, call);
        if (outcome.tag === "Outbound") {
          const buffer = packOutboundFrames(outcome.data);
          const response: EngineNetworkMessage = Tag("HostSettlement", {
            id,
            outcome: Tag("PackedOutbound", { buffer }),
          });
          this.#port.postMessage(response, [buffer]);
          return;
        }
        const response: EngineNetworkMessage = Tag("HostSettlement", {
          id,
          outcome,
        });
        this.#port.postMessage(response);
        return;
      }
      const outcome = await dispatchWorkerCapability(this.#engine, call);
      const response: EngineNetworkMessage = Tag("HostSettlement", {
        id,
        outcome,
      });
      this.#port.postMessage(response);
    } catch (error) {
      this.#fail(describeHostError(error));
    }
  }

  async #performIngressBatch(id: number, buffer: ArrayBuffer): Promise<void> {
    try {
      const items = unpackIngressItems(buffer);
      const outcomes = await Promise.all(
        items.map(({ interfaceId, bytes }) =>
          dispatchWorkerCapability(
            this.#engine,
            workerCapabilityCall("Ingest", { interfaceId, bytes }),
          )
        ),
      );
      const failures: PackedIngressFailure[] = [];
      for (let index = 0; index < outcomes.length; index += 1) {
        const outcome = outcomes[index];
        if (outcome !== undefined && outcome.tag !== "Accepted") {
          failures.push({ index, outcome });
        }
      }
      const response: EngineNetworkMessage = Tag("IngressSettled", {
        id,
        count: items.length,
        failures,
      });
      this.#port.postMessage(response);
    } catch (error) {
      this.#fail(describeHostError(error));
    }
  }

  #settle(id: number, operation: PendingNetworkCall["operation"], outcome: unknown): void {
    const pending = this.#pending.get(id);
    if (pending === undefined || pending.operation !== operation) {
      throw new Error(`network Worker settled unknown ${operation} call ${id}`);
    }
    this.#pending.delete(id);
    pending.settle(outcome);
  }

  #fail(detail: string): void {
    if (this.#failed) {
      return;
    }
    const running = this.#startSettled;
    this.#failed = true;
    if (!this.#startSettled && this.#startResolve !== undefined) {
      this.#startSettled = true;
      this.#startResolve(Tag("Failed", { detail }));
      this.#startResolve = undefined;
    }
    this.#failPending(detail);
    this.#port.close();
    this.#worker.terminate();
    if (running) {
      this.#networkFailed(detail);
    }
  }

  #failPending(detail: string): void {
    const error = new Error(detail);
    for (const pending of this.#pending.values()) {
      pending.fail(error);
    }
    this.#pending.clear();
  }
}

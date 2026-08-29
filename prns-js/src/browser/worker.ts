import { Tag, match } from "../casework.js";
import type { HostCommand, LinkId } from "../contract.js";
import { loadWasmModule } from "./bootstrap.js";
import { describeHostError } from "./host_errors.js";
import { describeInterfaceSessionFailure } from "./session.js";
import { Prns } from "./index.js";
import {
  bindWorkerEngineOptions,
  dispatchWorkerCapability,
} from "./worker_engine_bridge.js";
import type { InterfaceSession } from "./interface_contract.js";
import type {
  BrowserPersistedState,
  BrowserPersistenceStore,
  IdentityStore,
  StableIdentityStore,
} from "./persistence.js";
import type { SendResourceOptions, StopOutcome } from "./index.js";
import type { AutoWifiController } from "./auto_wifi/index.js";
import type {
  WorkerCall,
  WorkerCapabilityInvocation,
  WorkerCapabilityRequest,
  WorkerCapabilityResponse,
  WorkerCapabilitySettlement,
  WorkerControlRequest,
  WorkerControlResponse,
  WorkerInvocation,
  WorkerSettlement,
  WorkerStartMessage,
} from "./worker_protocol.js";
import { WORKER_WIRE_MAXIMUM_BYTES } from "./worker_protocol.js";
import { BoundedWorkerEventSender } from "./worker_event_sender.js";
import {
  BatchedPortReceiver,
  BatchedPortSender,
  messageTaskScheduler,
} from "../worker_wire/batched_port.js";
import { MAXIMUM_WIRE_BATCH_ITEMS } from "../worker_wire/wire_batch.js";

type StartedEngine = {
  readonly engine: Prns;
  readonly persistenceState: () => BrowserPersistedState | undefined;
};

const workerScope = globalThis as typeof globalThis & {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<WorkerStartMessage>) => void,
  ): void;
};

workerScope.addEventListener("message", (event) => {
  if (event.data.tag === "Initialize") {
    void initialize(event.data);
  }
});

async function initialize(message: WorkerStartMessage): Promise<void> {
  const { control, events, capabilities } = message.data;
  control.start();
  events.start();
  capabilities.start();
  let eventFlowFailed = false;
  const eventSender = new BoundedWorkerEventSender(
    events,
    message.data.initialization.limits,
    {
      protocol: (detail) => {
        if (!eventFlowFailed) {
          eventFlowFailed = true;
          postControl(control, Tag("ProtocolFailed", { detail }));
        }
      },
      backpressure: (rejectedEventBytes) => {
        if (!eventFlowFailed) {
          eventFlowFailed = true;
          postControl(control, Tag("EventBackpressureExceeded", {
            rejectedEventBytes,
          }));
        }
      },
    },
  );
  let started: Awaited<ReturnType<typeof startEngine>>;
  try {
    started = await startEngine(message, eventSender);
  } catch (error) {
    if (!eventFlowFailed) {
      postControl(control, Tag("ProtocolFailed", {
        detail: describeHostError(error),
      }));
    }
    return;
  }
  if (started.tag !== "Ready") {
    postControl(control, Tag("Started", { outcome: started }));
    return;
  }
  const state = started.data;
  const sessions = new Map<number, InterfaceSession>();
  const autoWifi: { controller: AutoWifiController | undefined } = {
    controller: undefined,
  };
  let nextSessionId = 1;
  postControl(control, Tag("Started", {
    outcome: Tag("Ready", {
      backendInfo: state.engine.backendInfo,
      lifecycle: state.engine.lifecycle,
    }),
  }));
  const settlementSender = new BatchedPortSender<WorkerSettlement>({
    port: control,
    wrap: (batch) => Tag("Settlements", { batch }),
    maximumItems: MAXIMUM_WIRE_BATCH_ITEMS,
    maximumBytes: WORKER_WIRE_MAXIMUM_BYTES,
    scheduleTask: messageTaskScheduler(),
    failed: (error) => {
      postControl(control, Tag("ProtocolFailed", {
        detail: describeHostError(error),
      }));
    },
  });
  const capabilitySettlementSender = new BatchedPortSender<WorkerCapabilitySettlement>({
    port: capabilities,
    wrap: (batch) => Tag("CapabilitySettlements", { batch }),
    maximumItems: MAXIMUM_WIRE_BATCH_ITEMS,
    maximumBytes: WORKER_WIRE_MAXIMUM_BYTES,
    scheduleTask: messageTaskScheduler(),
    failed: (error) => {
      postControl(capabilities, Tag("ProtocolFailed", {
        detail: describeHostError(error),
      }));
    },
  });
  const callReceiver = new BatchedPortReceiver<WorkerInvocation>((invocation) => {
    void settleCall(invocation);
  });
  const capabilityReceiver = new BatchedPortReceiver<WorkerCapabilityInvocation>((invocation) => {
    void settleCapability(invocation);
  });
  control.addEventListener("message", (request: MessageEvent<WorkerControlRequest>) => {
    if (request.data.tag !== "Calls") {
      postControl(control, Tag("ProtocolFailed", {
        detail: "worker control channel received an unknown message",
      }));
      return;
    }
    try {
      callReceiver.receive(request.data.data.batch);
    } catch (error) {
      postControl(control, Tag("ProtocolFailed", {
        detail: describeHostError(error),
      }));
    }
  });
  capabilities.addEventListener("message", (request: MessageEvent<WorkerCapabilityRequest>) => {
    if (request.data.tag !== "CapabilityCalls") {
      postControl(capabilities, Tag("ProtocolFailed", {
        detail: "worker capability channel received an unknown message",
      }));
      return;
    }
    try {
      capabilityReceiver.receive(request.data.data.batch);
    } catch (error) {
      postControl(capabilities, Tag("ProtocolFailed", {
        detail: describeHostError(error),
      }));
    }
  });

  async function settleCall(request: WorkerInvocation): Promise<void> {
    try {
      const callOutcome = await performCall(
        state.engine,
        request.call,
        sessions,
        () => nextSessionId++,
        autoWifi,
      );
      const outcome = request.call.tag === "Stop"
        ? {
            stopOutcome: callOutcome,
            persistedState: state.persistenceState(),
            snapshot: await state.engine.snapshot(),
            hostSnapshot: await state.engine.hostSnapshot(),
          }
        : callOutcome;
      settlementSender.send({
        id: request.id,
        call: request.call.tag,
        outcome,
      });
    } catch (error) {
      postControl(control, Tag("ProtocolFailed", {
        id: request.id,
        detail: describeHostError(error),
      }));
    }
  }

  async function settleCapability(request: WorkerCapabilityInvocation): Promise<void> {
    try {
      const outcome = await dispatchWorkerCapability(state.engine, request.call);
      capabilitySettlementSender.send({
        id: request.id,
        call: request.call.tag,
        outcome,
      });
    } catch (error) {
      postControl(capabilities, Tag("ProtocolFailed", {
        id: request.id,
        detail: describeHostError(error),
      }));
    }
  }
}

async function startEngine(
  message: WorkerStartMessage,
  events: BoundedWorkerEventSender,
): Promise<ReturnType<typeof Tag<"Ready", StartedEngine>> | Exclude<Awaited<ReturnType<typeof Prns.create>>, { readonly tag: "Ready" }>> {
  const initialization = message.data.initialization;
  const identityStore: IdentityStore = {
    load: async () => Tag("Loaded", initialization.identity),
    save: async () => Tag("Saved"),
  };
  const bleIdentityStore: StableIdentityStore = initialization.bleIdentity === undefined
    ? {
        load: async () => Tag("Missing"),
        save: async () => Tag("Saved"),
      }
    : {
        load: async () => Tag("Loaded", initialization.bleIdentity as Uint8Array),
        save: async () => Tag("Saved"),
      };
  let savedState: BrowserPersistedState | undefined;
  const persistenceStore: BrowserPersistenceStore | undefined = initialization.persistenceEnabled
    ? {
        load: async () => initialization.persistedState === undefined
          ? Tag("Missing")
          : Tag("Loaded", initialization.persistedState),
        save: async (state) => {
          savedState = state;
          return Tag("Saved");
        },
      }
    : undefined;
  const loaded = initialization.wasmModuleUrl === undefined
    ? undefined
    : await loadWasmModule(new URL(initialization.wasmModuleUrl));
  if (loaded !== undefined && loaded.tag !== "Loaded") {
    return loaded;
  }
  const engineOptions = bindWorkerEngineOptions(
    {
      execution: "MainThread",
      ...(loaded === undefined ? {} : { wasm: loaded.data }),
      identityStore,
      bleIdentityStore,
      ...(persistenceStore === undefined ? {} : { persistenceStore }),
      limits: initialization.limits,
      ...(initialization.resourceCompressionModuleUrl === undefined
        ? {}
        : {
            resourceCompressionModuleUrl: new URL(
              initialization.resourceCompressionModuleUrl,
            ),
          }),
    },
    {
      eventBatchSink: (batch) => {
        events.sendBatch(batch);
      },
      directDiagnosticSink: (diagnostic) => {
        events.sendDiagnostic(diagnostic);
      },
      ...(initialization.autoWifiSelectionSeed === undefined
        ? {}
        : { autoWifiSelectionSeed: initialization.autoWifiSelectionSeed }),
    },
  );
  const created = await Prns.create(engineOptions);
  if (created.tag !== "Ready") {
    return created;
  }
  return Tag("Ready", {
    engine: created.data,
    persistenceState: () => savedState,
  });
}

async function performCall(
  engine: Prns,
  call: WorkerCall,
  sessions: Map<number, InterfaceSession>,
  mintSessionId: () => number,
  autoWifi: { controller: AutoWifiController | undefined },
): Promise<unknown> {
  return match(call, {
    RegisterSingleDestination: (options) => engine.registerSingleDestination(options),
    RegisterNodePage: (appData) => engine.registerNodePage(appData),
    Execute: (command) => engine.execute(command as HostCommand),
    SendResourceBlob: ({ linkId, blob, options }) => engine.sendResourceBlob(
      linkId as LinkId,
      blob,
      options as SendResourceOptions,
    ),
    Snapshot: () => engine.snapshot(),
    HostSnapshot: () => engine.hostSnapshot(),
    Stop: () => stopEngine(engine, sessions, autoWifi),
    WebSocketConnect: async ({ url, options }) => {
      const outcome = await engine.interfaces.webSocket.connect(url, options);
      if (outcome.tag !== "Connected") {
        return outcome;
      }
      const id = mintSessionId();
      sessions.set(id, outcome.data);
      return Tag("Connected", {
        id,
        name: outcome.data.name,
        interfaceId: outcome.data.interfaceId,
        status: outcome.data.status,
        url: outcome.data.url,
        framing: outcome.data.framing,
      });
    },
    AutoWifiStart: () => {
      autoWifi.controller = engine.interfaces.autoWifi.start();
      return autoWifi.controller.status;
    },
    AutoWifiStatus: () => autoWifi.controller?.status ?? Tag("Closed"),
    AutoWifiClose: () => {
      const controller = autoWifi.controller;
      autoWifi.controller = undefined;
      return controller?.close() ?? Tag("Closed");
    },
    InterfaceSessionClose: (id) => {
      const session = sessions.get(id);
      if (session === undefined) {
        return Tag("Closed");
      }
      sessions.delete(id);
      return session.close();
    },
  });
}

async function stopEngine(
  engine: Prns,
  sessions: Map<number, InterfaceSession>,
  autoWifi: { controller: AutoWifiController | undefined },
): Promise<StopOutcome> {
  const failures: string[] = [];
  const activeSessions = [...sessions.values()];
  sessions.clear();
  const sessionOutcomes = await Promise.all(
    activeSessions.map(async (session): Promise<string | undefined> => {
      try {
        const outcome = await session.close();
        return outcome.tag === "Closed"
          ? undefined
          : describeInterfaceSessionFailure(outcome);
      } catch (error) {
        return describeHostError(error);
      }
    }),
  );
  failures.push(
    ...sessionOutcomes.filter(
      (outcome): outcome is string => outcome !== undefined,
    ),
  );
  const autoWifiController = autoWifi.controller;
  autoWifi.controller = undefined;
  if (autoWifiController !== undefined) {
    try {
      const closed = await autoWifiController.close();
      if (closed.tag === "RuntimeRejected") {
        failures.push(closed.data.detail);
      }
    } catch (error) {
      failures.push(describeHostError(error));
    }
  }
  const stopped = await engine.stop();
  if (stopped.tag === "OperationFailed") {
    failures.push(stopped.data.detail);
  }
  if (failures.length === 0) {
    return stopped;
  }
  return Tag("OperationFailed", {
    operation: "stop",
    detail: failures.join("; "),
  });
}

function postControl(
  port: MessagePort,
  message: WorkerControlResponse | WorkerCapabilityResponse,
): void {
  port.postMessage(message);
}

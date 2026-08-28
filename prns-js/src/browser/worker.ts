import { Tag } from "../casework.js";
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
  WorkerCapabilityRequest,
  WorkerControlRequest,
  WorkerControlResponse,
  WorkerStartMessage,
} from "./worker_protocol.js";
import { BoundedWorkerEventSender } from "./worker_event_sender.js";

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
  if (event.data.type === "initialize") {
    void initialize(event.data);
  }
});

async function initialize(message: WorkerStartMessage): Promise<void> {
  const { control, events, capabilities } = message;
  control.start();
  events.start();
  capabilities.start();
  let eventFlowFailed = false;
  const eventSender = new BoundedWorkerEventSender(
    events,
    message.initialization.limits,
    {
      protocol: (detail) => {
        if (!eventFlowFailed) {
          eventFlowFailed = true;
          postControl(control, { type: "protocolFailed", detail });
        }
      },
      backpressure: (rejectedEventBytes) => {
        if (!eventFlowFailed) {
          eventFlowFailed = true;
          postControl(control, {
            type: "eventBackpressureExceeded",
            rejectedEventBytes,
          });
        }
      },
    },
  );
  let started: Awaited<ReturnType<typeof startEngine>>;
  try {
    started = await startEngine(message, eventSender);
  } catch (error) {
    if (!eventFlowFailed) {
      postControl(control, {
        type: "protocolFailed",
        detail: describeHostError(error),
      });
    }
    return;
  }
  if (started.tag !== "Ready") {
    postControl(control, { type: "started", outcome: started });
    return;
  }
  const state = started.data;
  const sessions = new Map<number, InterfaceSession>();
  const autoWifi: { controller: AutoWifiController | undefined } = {
    controller: undefined,
  };
  let nextSessionId = 1;
  postControl(control, {
    type: "started",
    outcome: Tag("Ready", {
      backendInfo: state.engine.backendInfo,
      lifecycle: state.engine.lifecycle,
    }),
  });
  control.addEventListener("message", (request: MessageEvent<WorkerControlRequest>) => {
    void settleCall(request.data);
  });
  capabilities.addEventListener("message", (request: MessageEvent<WorkerCapabilityRequest>) => {
    void settleCapability(request.data);
  });

  async function settleCall(request: WorkerControlRequest): Promise<void> {
    if (request.type !== "call") {
      postControl(control, {
        type: "protocolFailed",
        detail: "worker control channel received an unknown message",
      });
      return;
    }
    try {
      const callOutcome = await performCall(
        state.engine,
        request.call,
        sessions,
        () => nextSessionId++,
        autoWifi,
      );
      const outcome = request.call.operation === "stop"
        ? {
            stopOutcome: callOutcome,
            persistedState: state.persistenceState(),
            snapshot: await state.engine.snapshot(),
            hostSnapshot: await state.engine.hostSnapshot(),
          }
        : callOutcome;
      postControl(control, {
        type: "settled",
        id: request.id,
        outcome,
      });
    } catch (error) {
      postControl(control, {
        type: "protocolFailed",
        id: request.id,
        detail: describeHostError(error),
      });
    }
  }

  async function settleCapability(request: WorkerCapabilityRequest): Promise<void> {
    if (request.type !== "call") {
      postControl(capabilities, {
        type: "protocolFailed",
        detail: "worker capability channel received an unknown message",
      });
      return;
    }
    try {
      const outcome = await dispatchWorkerCapability(state.engine, request.call);
      postControl(capabilities, {
        type: "settled",
        id: request.id,
        outcome,
      });
    } catch (error) {
      postControl(capabilities, {
        type: "protocolFailed",
        id: request.id,
        detail: describeHostError(error),
      });
    }
  }
}

async function startEngine(
  message: WorkerStartMessage,
  events: BoundedWorkerEventSender,
): Promise<ReturnType<typeof Tag<"Ready", StartedEngine>> | Exclude<Awaited<ReturnType<typeof Prns.create>>, { readonly tag: "Ready" }>> {
  const initialization = message.initialization;
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
  switch (call.operation) {
    case "registerSingleDestination":
      return engine.registerSingleDestination(call.value);
    case "registerNodePage":
      return engine.registerNodePage(call.value);
    case "execute":
      return engine.execute(call.value as HostCommand);
    case "sendResourceBlob":
      return engine.sendResourceBlob(
        call.value.linkId as LinkId,
        call.value.blob,
        call.value.options as SendResourceOptions,
      );
    case "snapshot":
      return engine.snapshot();
    case "hostSnapshot":
      return engine.hostSnapshot();
    case "stop":
      return stopEngine(engine, sessions, autoWifi);
    case "webSocketConnect": {
      const outcome = await engine.interfaces.webSocket.connect(
        call.value.url,
        call.value.options as Parameters<typeof engine.interfaces.webSocket.connect>[1],
      );
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
    }
    case "autoWifiStart":
      autoWifi.controller = engine.interfaces.autoWifi.start();
      return autoWifi.controller.status;
    case "autoWifiStatus":
      return autoWifi.controller?.status ?? Tag("Closed");
    case "autoWifiClose": {
      const controller = autoWifi.controller;
      autoWifi.controller = undefined;
      return controller?.close() ?? Tag("Closed");
    }
    case "interfaceSessionClose": {
      const session = sessions.get(call.value);
      if (session === undefined) {
        return Tag("Closed");
      }
      sessions.delete(call.value);
      return session.close();
    }
  }
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

function postControl(port: MessagePort, message: WorkerControlResponse): void {
  port.postMessage(message);
}

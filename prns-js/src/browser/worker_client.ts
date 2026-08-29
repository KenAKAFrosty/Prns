import { Tag, match_into } from "../casework.js";
import { BoundedAsyncLane } from "../async_lanes.js";
import {
  IDENTITY_SECRET_LENGTH,
  balancedLimits,
} from "../contract.js";
import type {
  BackendCapabilities,
  BackendInfo,
  CommandSettlement,
  CommandSettlementFor,
  DestinationHash,
  HostCommand,
  HostSnapshot,
  IdentityHash,
  InterfaceConfig,
  InterfaceId,
  InterfaceRoutingPolicy,
  LifecycleState,
  LinkId,
  RequestId,
  RequestPathHash,
  ResourceCompression,
  ResourceHash,
  ResourceStrategy,
  ResponseTimeout,
} from "../contract.js";
import type { LanePushOutcome, StreamClaim } from "../async_lanes.js";
import {
  browserLimits,
  loadBundledWasm,
  loadWasmModule,
  loadOrCreateBleIdentity,
  webCryptoIdentity,
} from "./bootstrap.js";
import { commandFailed } from "./command_settlement.js";
import {
  parseEventBatch,
  retainedApplicationEventBytes,
} from "./events.js";
import type {
  PrnsApplicationEvent,
  PrnsDiagnosticEvent,
} from "./events.js";
import { describeHostError } from "./host_errors.js";
import type {
  InterfaceCloseOutcome,
  InterfaceSession,
  InterfaceSessionStatus,
} from "./interface_contract.js";
import { describeInterfaceSessionFailure } from "./session.js";
import {
  BrowserLocalStorageBleIdentityStore,
  describePersistenceStoreFailure,
  parseBrowserPersistedState,
} from "./persistence.js";
import type {
  BrowserPersistenceStore,
  IdentityLoadOutcome,
  IdentitySaveOutcome,
  PersistenceLoadOutcome,
} from "./persistence.js";
import type {
  DedicatedWorkerPrnsOptions,
  DestinationRegistrationOutcome,
  HostSnapshotOutcome,
  Prns,
  PrnsCreateOutcome,
  SendResourceOptions,
  SnapshotOutcome,
  StopOutcome,
} from "./index.js";
import type {
  BleIdentityAvailability,
  PrnsWasmModule,
  RegisterSingleDestinationOptions,
} from "./runtime_contract.js";
import { BluetoothInterface } from "./bluetooth/index.js";
import type { BluetoothRuntimeHost } from "./bluetooth/runtime.js";
import { UsbAutoInterface } from "./usb_auto/index.js";
import type { UsbAutoRuntimeHost } from "./usb_auto/runtime.js";
import { RNodeInterface } from "./rnode.js";
import type { PrnsInterfaces } from "./interfaces.js";
import { byteKey } from "./bytes.js";
import { bitrateBps, hardwareMtu, packetFrame } from "./values.js";
import type { PrnsSnapshot } from "./snapshot.js";
import {
  identitySecretKey,
} from "./values.js";
import type { IdentitySecretKey } from "./values.js";
import type {
  WebSocketConnectOptions,
  WebSocketConnectOutcome,
  WebSocketSession,
} from "./websocket/index.js";
import type {
  AutoWifiControllerCloseOutcome,
  AutoWifiControllerStatus,
} from "./auto_wifi/index.js";
import { loadOrCreateAutoWifiSelectionSeed } from "./auto_wifi/index.js";
import type {
  WorkerCall,
  WorkerCallOutcome,
  WorkerCapabilityCall,
  WorkerCapabilityCallOutcome,
  WorkerCapabilityResponse,
  WorkerCapabilitySettlement,
  WorkerControlResponse,
  WorkerEventRequest,
  WorkerEventMessage,
  WorkerInitialization,
  WorkerInvocation,
  WorkerProjectionMessage,
  WorkerProjectionRequest,
  WorkerProjectionUpdate,
  WorkerSettlement,
  WorkerSessionProjection,
  WorkerShutdownResponse,
  WorkerShutdownState,
  WorkerStartMessage,
  WorkerReadyState,
} from "./worker_protocol.js";
import {
  MAXIMUM_PENDING_PROJECTION_SYNCHRONIZATIONS,
  WORKER_WIRE_MAXIMUM_BYTES,
  workerCall,
  workerCapabilityCall,
} from "./worker_protocol.js";
import {
  BatchedPortReceiver,
  BatchedPortSender,
  messageTaskScheduler,
} from "../worker_wire/batched_port.js";
import { MAXIMUM_WIRE_BATCH_ITEMS } from "../worker_wire/wire_batch.js";
import {
  MINIMUM_WORKER_CODEC_ITEMS,
  workerInvocationCodec,
  workerInvocationWireBytes,
  workerSettlementCodec,
} from "./worker_codecs.js";
import { PrnsProjectionStore } from "./projections.js";
import type {
  PrnsProjection,
  PrnsProjectionValue,
  PrnsView,
  ProjectionSynchronization,
} from "./projections.js";
import {
  parseProjectionSynchronization,
  parseWorkerDiagnosticEvent,
  parseWorkerProjectionUpdate,
} from "./worker_projection_validation.js";

type WorkerPreparation = {
  readonly initialization: WorkerInitialization;
  readonly persistenceStore?: BrowserPersistenceStore;
  readonly wasm: PrnsWasmModule;
  readonly bleIdentityAvailability: BleIdentityAvailability;
};

type PendingCall = {
  readonly settle: (outcome: unknown) => void;
  readonly fail: (error: Error) => void;
};

type PendingControlCall = PendingCall & {
  readonly call: WorkerCall;
};

type PendingCapabilityCall = PendingCall & {
  readonly call: WorkerCapabilityCall;
};

type PendingProjectionSynchronization = {
  readonly view: PrnsView;
  readonly settle: (outcome: ProjectionSynchronization<unknown>) => void;
};

const WORKER_START_TIMEOUT_MILLIS = 10_000;
const AUTO_WIFI_STATUS_POLL_MILLIS = 250;
const MAXIMUM_PENDING_CONTROL_CALLS = 32;

export async function createDedicatedWorkerPrns(
  options: DedicatedWorkerPrnsOptions,
): Promise<PrnsCreateOutcome> {
  if (typeof globalThis.Worker !== "function") {
    return Tag("WorkerStartFailed", {
      detail: "DedicatedWorker is not available in this browser context",
    });
  }
  const prepared = await prepareWorker(options);
  if (prepared.tag !== "Prepared") {
    return prepared;
  }
  let worker: Worker;
  try {
    worker = new Worker(new URL("./worker.js", import.meta.url), {
      type: "module",
      name: "prns-engine",
    });
  } catch (error) {
    return Tag("WorkerStartFailed", { detail: describeHostError(error) });
  }
  const controlChannel = new MessageChannel();
  const eventChannel = new MessageChannel();
  const capabilityChannel = new MessageChannel();
  const projectionChannel = new MessageChannel();
  const shutdownChannel = new MessageChannel();
  const client = new DedicatedWorkerPrns(
    worker,
    controlChannel.port1,
    eventChannel.port1,
    capabilityChannel.port1,
    projectionChannel.port1,
    shutdownChannel.port1,
    prepared.data.initialization.limits,
    prepared.data.persistenceStore,
    prepared.data.wasm,
    prepared.data.bleIdentityAvailability,
  );
  const started = client.started();
  const message: WorkerStartMessage = Tag("Initialize", {
    initialization: prepared.data.initialization,
    control: controlChannel.port2,
    events: eventChannel.port2,
    capabilities: capabilityChannel.port2,
    projections: projectionChannel.port2,
    shutdown: shutdownChannel.port2,
  });
  worker.postMessage(message, [
    controlChannel.port2,
    eventChannel.port2,
    capabilityChannel.port2,
    projectionChannel.port2,
    shutdownChannel.port2,
  ]);
  const outcome = await started;
  if (outcome.tag !== "Ready") {
    client.terminate();
    return outcome;
  }
  client.acceptStart(outcome.data);
  return Tag("Ready", client as unknown as Prns);
}

class DedicatedWorkerPrns {
  readonly interfaces: PrnsInterfaces;
  readonly #worker: Worker;
  readonly #control: MessagePort;
  readonly #eventsPort: MessagePort;
  readonly #capabilitiesPort: MessagePort;
  readonly #projectionsPort: MessagePort;
  readonly #shutdownPort: MessagePort;
  readonly #limits: WorkerInitialization["limits"];
  readonly #persistenceStore: BrowserPersistenceStore | undefined;
  readonly #events: BoundedAsyncLane<PrnsApplicationEvent>;
  readonly #diagnostics: BoundedAsyncLane<PrnsDiagnosticEvent>;
  readonly #controlSender: BatchedPortSender<WorkerInvocation>;
  readonly #capabilitySender: BatchedPortSender<{
    readonly id: number;
    readonly call: WorkerCapabilityCall;
  }>;
  readonly #controlSettlements: BatchedPortReceiver<WorkerSettlement>;
  readonly #capabilitySettlements: BatchedPortReceiver<WorkerCapabilitySettlement>;
  readonly #projectionUpdates: BatchedPortReceiver<WorkerProjectionUpdate>;
  readonly #pending = new Map<number, PendingControlCall>();
  readonly #ignoredSettlements = new Set<number>();
  readonly #capabilityPending = new Map<number, PendingCapabilityCall>();
  readonly #pendingProjectionSynchronizations = new Map<
    number,
    PendingProjectionSynchronization
  >();
  readonly #pageSessions = new Map<string, InterfaceSession>();
  readonly #webSocketSessions = new Map<number, WorkerWebSocketSession>();
  readonly #autoWifi: WorkerAutoWifiInterface;
  #nextCallId = 1;
  #nextProjectionSynchronizationId = 1;
  #startSettled = false;
  #startTimer: number | undefined;
  #startResolve:
    | ((outcome: Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }> | ReturnType<typeof readyWorker>) => void)
    | undefined;
  #lifecycle: LifecycleState = Tag("Starting");
  #backendInfo: BackendInfo | undefined;
  #stopPromise: Promise<StopOutcome> | undefined;
  #stopCompleted = false;
  #terminated = false;
  #stoppedSnapshot: SnapshotOutcome | undefined;
  #stoppedHostSnapshot: HostSnapshotOutcome | undefined;
  #persistenceFailureDetail: string | undefined;
  #projections: PrnsProjectionStore | undefined;
  #pendingCommandCount = 0;
  #pendingControlCount = 0;
  #shutdownResolve: ((state: WorkerShutdownState) => void) | undefined;
  #shutdownReject: ((error: Error) => void) | undefined;

  constructor(
    worker: Worker,
    control: MessagePort,
    events: MessagePort,
    capabilities: MessagePort,
    projections: MessagePort,
    shutdown: MessagePort,
    limits: WorkerInitialization["limits"],
    persistenceStore: BrowserPersistenceStore | undefined,
    wasm: PrnsWasmModule,
    bleIdentityAvailability: BleIdentityAvailability,
  ) {
    this.#worker = worker;
    this.#control = control;
    this.#eventsPort = events;
    this.#capabilitiesPort = capabilities;
    this.#projectionsPort = projections;
    this.#shutdownPort = shutdown;
    this.#limits = limits;
    this.#persistenceStore = persistenceStore;
    this.#events = new BoundedAsyncLane({
      name: "ApplicationEvents",
      maximumValues: limits.applicationEvents,
      maximumBytes: limits.retainedEventBytes,
      measure: retainedApplicationEventBytes,
      onRejected: (rejectedEventBytes) => this.#failBackpressure(rejectedEventBytes),
    });
    this.#diagnostics = new BoundedAsyncLane<PrnsDiagnosticEvent>({
      name: "Diagnostics",
      maximumValues: limits.diagnostics,
      maximumBytes: Number.MAX_SAFE_INTEGER,
      measure: () => 0,
      gap: (count) => Tag("DiagnosticsDropped", { count }),
    });
    this.#controlSender = new BatchedPortSender({
      port: control,
      wrap: (batch) => Tag("Calls", { batch }),
      maximumItems: Math.min(MAXIMUM_WIRE_BATCH_ITEMS, limits.pendingCommands),
      maximumQueuedItems: limits.pendingCommands + MAXIMUM_PENDING_CONTROL_CALLS,
      maximumBytes: WORKER_WIRE_MAXIMUM_BYTES,
      measureBytes: workerInvocationWireBytes,
      scheduleTask: messageTaskScheduler(),
      failed: (error) => this.#failProtocol(describeHostError(error)),
      codec: workerInvocationCodec,
      codecPolicy: { minimumCodecItems: MINIMUM_WORKER_CODEC_ITEMS },
    });
    this.#capabilitySender = new BatchedPortSender({
      port: capabilities,
      wrap: (batch) => Tag("CapabilityCalls", { batch }),
      maximumItems: Math.min(MAXIMUM_WIRE_BATCH_ITEMS, limits.pendingCommands),
      maximumQueuedItems: limits.pendingCommands,
      maximumBytes: WORKER_WIRE_MAXIMUM_BYTES,
      measureBytes: workerCapabilityInvocationWireBytes,
      scheduleTask: messageTaskScheduler(),
      failed: (error) => this.#failProtocol(describeHostError(error)),
    });
    this.#controlSettlements = new BatchedPortReceiver(
      (settlement) => this.#receiveControlSettlement(settlement),
      [workerSettlementCodec],
    );
    this.#capabilitySettlements = new BatchedPortReceiver((settlement) => {
      this.#receiveCapabilitySettlement(settlement);
    });
    this.#projectionUpdates = new BatchedPortReceiver((update) => {
      this.#applyProjectionUpdate(parseWorkerProjectionUpdate(update));
    });
    const capabilityHost = new WorkerCapabilityHost(
      wasm,
      bleIdentityAvailability,
      () => this.#lifecycle,
      (call) => this.#capabilityCall(call),
    );
    this.#autoWifi = new WorkerAutoWifiInterface(this);
    this.interfaces = {
      webSocket: new WorkerWebSocketInterface(this),
      bluetooth: new BluetoothInterface(capabilityHost, (session) =>
        this.#pageSessions.set(byteKey(session.interfaceId), session),
      ),
      usbAuto: new UsbAutoInterface(capabilityHost, (session) =>
        this.#pageSessions.set(byteKey(session.interfaceId), session),
      ),
      rnode: new RNodeInterface(capabilityHost),
      autoWifi: this.#autoWifi,
    } as unknown as PrnsInterfaces;
    control.addEventListener("message", (event: MessageEvent<WorkerControlResponse>) => {
      this.#receiveWorkerMessage(event.data, (message) => {
        this.#receiveControl(message as WorkerControlResponse);
      });
    });
    events.addEventListener("message", (event: MessageEvent<WorkerEventMessage>) => {
      this.#receiveWorkerMessage(event.data, (message) => {
        this.#receiveEvent(message as WorkerEventMessage);
      });
    });
    capabilities.addEventListener("message", (event: MessageEvent<WorkerCapabilityResponse>) => {
      this.#receiveWorkerMessage(event.data, (message) => {
        this.#receiveCapability(message as WorkerCapabilityResponse);
      });
    });
    projections.addEventListener(
      "message",
      (event: MessageEvent<WorkerProjectionMessage>) => {
        this.#receiveWorkerMessage(event.data, (message) => {
          this.#receiveProjection(message as WorkerProjectionMessage);
        });
      },
    );
    shutdown.addEventListener(
      "message",
      (event: MessageEvent<WorkerShutdownResponse>) => {
        this.#receiveWorkerMessage(event.data, (message) => {
          this.#receiveShutdown(message as WorkerShutdownResponse);
        });
      },
    );
    control.start();
    events.start();
    capabilities.start();
    projections.start();
    shutdown.start();
    worker.addEventListener("error", (event) => {
      this.#failProtocol(event.message || "DedicatedWorker failed");
    });
    worker.addEventListener("messageerror", () => {
      this.#failProtocol("DedicatedWorker message could not be decoded");
    });
  }

  started(): Promise<Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }> | ReturnType<typeof readyWorker>> {
    return new Promise((resolve) => {
      this.#startResolve = resolve;
      this.#startTimer = globalThis.setTimeout(() => {
        if (!this.#startSettled) {
          this.#settleStart(Tag("WorkerStartFailed", {
            detail: `DedicatedWorker did not start within ${WORKER_START_TIMEOUT_MILLIS} milliseconds`,
          }));
        }
      }, WORKER_START_TIMEOUT_MILLIS);
    });
  }

  acceptStart(data: WorkerReadyState): void {
    this.#backendInfo = data.backendInfo;
    this.#lifecycle = data.lifecycle;
    this.#projections = new PrnsProjectionStore(
      data.hostSnapshot,
      data.lifecycle,
      this.#limits.diagnostics,
      {
        observed: (view) => {
          if (view.tag !== "Diagnostics") {
            this.#sendProjectionRequest(Tag("Observe", { view }));
          }
        },
        unobserved: (view) => {
          if (view.tag !== "Diagnostics") {
            this.#sendProjectionRequest(Tag("Unobserve", { view }));
          }
        },
        synchronize: (view) => this.#synchronizeProjection(view),
        diagnosticCapacityChanged: (maximumEvents) =>
          this.#sendProjectionRequest(Tag("ObserveDiagnostics", { maximumEvents })),
      },
    );
  }

  terminate(): void {
    if (this.#terminated) {
      return;
    }
    this.#terminated = true;
    if (this.#startTimer !== undefined) {
      globalThis.clearTimeout(this.#startTimer);
      this.#startTimer = undefined;
    }
    this.#controlSender.fail();
    this.#capabilitySender.fail();
    this.#control.close();
    this.#eventsPort.close();
    this.#capabilitiesPort.close();
    this.#projectionsPort.close();
    this.#shutdownPort.close();
    this.#worker.terminate();
  }

  async registerSingleDestination(
    options: RegisterSingleDestinationOptions,
  ): Promise<DestinationRegistrationOutcome> {
    return this.#call(workerCall("RegisterSingleDestination", options));
  }

  async registerNodePage(appData: Uint8Array): Promise<DestinationRegistrationOutcome> {
    return this.#call(workerCall("RegisterNodePage", appData));
  }

  execute<Command extends HostCommand>(
    command: Command,
  ): Promise<CommandSettlementFor<Command>> {
    if (this.#pendingCommandCount >= this.#limits.pendingCommands) {
      return Promise.resolve(commandFailed(Tag("Busy")) as CommandSettlementFor<Command>);
    }
    if (this.#lifecycle.tag !== "Running") {
      return Promise.resolve(commandFailed(Tag("NodeStopped")) as CommandSettlementFor<Command>);
    }
    return this.#call(workerCall("Execute", command)) as Promise<CommandSettlementFor<Command>>;
  }

  announce(destination: DestinationHash, interfaceId?: InterfaceId): Promise<CommandSettlement> {
    return this.execute(Tag("Announce", interfaceId === undefined
      ? { destination }
      : { destination, interface: interfaceId }));
  }

  sendSinglePacket(destination: DestinationHash, payload: Uint8Array): Promise<CommandSettlement> {
    return this.execute(Tag("SendSinglePacket", { destination, payload }));
  }

  closeLink(linkId: LinkId): Promise<CommandSettlement> {
    return this.execute(Tag("CloseLink", { linkId }));
  }

  attachInterface(
    config: InterfaceConfig,
    routing?: InterfaceRoutingPolicy,
  ): Promise<CommandSettlement> {
    return this.execute(
      routing === undefined
        ? Tag("AttachInterface", { config })
        : Tag("AttachInterface", { config, routing }),
    );
  }

  detachInterface(interfaceId: InterfaceId): Promise<CommandSettlement> {
    return this.execute(Tag("DetachInterface", { interface: interfaceId }));
  }

  establishLink(destination: DestinationHash): Promise<CommandSettlement> {
    return this.execute(Tag("EstablishLink", { destination }));
  }

  requestPath(destination: DestinationHash): Promise<CommandSettlement> {
    return this.execute(Tag("RequestPath", { destination }));
  }

  identify(linkId: LinkId, identity: IdentityHash): Promise<CommandSettlement> {
    return this.execute(Tag("Identify", { linkId, identity }));
  }

  sendLinkPacket(linkId: LinkId, payload: Uint8Array): Promise<CommandSettlement> {
    return this.execute(Tag("SendLinkPacket", { linkId, payload }));
  }

  request(
    linkId: LinkId,
    pathHash: RequestPathHash,
    payload: Uint8Array,
    timeout: ResponseTimeout = Tag("LinkDefault"),
    maximumResponseBytes?: number,
  ): Promise<CommandSettlement> {
    return this.execute(Tag("Request", maximumResponseBytes === undefined
      ? { linkId, pathHash, payload, timeout }
      : { linkId, pathHash, payload, timeout, maximumResponseBytes }));
  }

  respond(
    linkId: LinkId,
    requestId: RequestId,
    requestRttMillis: number,
    payload: Uint8Array,
  ): Promise<CommandSettlement> {
    return this.execute(Tag("Respond", { linkId, requestId, requestRttMillis, payload }));
  }

  sendResource(
    linkId: LinkId,
    payload: Uint8Array,
    options: SendResourceOptions = {},
  ): Promise<CommandSettlement> {
    return this.execute(Tag("SendResource", {
      linkId,
      payload,
      ...(options.packedMetadata === undefined ? {} : { packedMetadata: options.packedMetadata }),
      compression: options.compression ?? Tag("Auto"),
    }));
  }

  sendResourceBlob(
    linkId: LinkId,
    blob: Blob,
    options: SendResourceOptions = {},
  ): Promise<CommandSettlement> {
    if (this.#pendingCommandCount >= this.#limits.pendingCommands) {
      return Promise.resolve(commandFailed(Tag("Busy")));
    }
    if (this.#lifecycle.tag !== "Running") {
      return Promise.resolve(commandFailed(Tag("NodeStopped")));
    }
    return this.#call(workerCall("SendResourceBlob", { linkId, blob, options }));
  }

  setLinkResourceStrategy(linkId: LinkId, strategy: ResourceStrategy): Promise<CommandSettlement> {
    return this.execute(Tag("SetLinkResourceStrategy", { linkId, strategy }));
  }

  setDestinationResourceStrategy(
    destination: DestinationHash,
    strategy: ResourceStrategy,
  ): Promise<CommandSettlement> {
    return this.execute(Tag("SetDestinationResourceStrategy", { destination, strategy }));
  }

  sendChannelMessage(
    linkId: LinkId,
    messageType: number,
    payload: Uint8Array,
  ): Promise<CommandSettlement> {
    return this.execute(Tag("SendChannelMessage", { linkId, messageType, payload }));
  }

  allowRequester(
    destination: DestinationHash,
    pathHash: RequestPathHash,
    identity: IdentityHash,
  ): Promise<CommandSettlement> {
    return this.execute(Tag("AllowRequester", { destination, pathHash, identity }));
  }

  get lifecycle(): LifecycleState {
    return this.#lifecycle;
  }

  get execution(): "DedicatedWorker" {
    return "DedicatedWorker";
  }

  get backendInfo(): BackendInfo {
    if (this.#backendInfo === undefined) {
      throw new Error("DedicatedWorker backend information is not ready");
    }
    return this.#backendInfo;
  }

  get capabilities(): BackendCapabilities {
    const info = this.backendInfo;
    return Tag("Cooperative", {
      available: new Set(info.capabilities),
      interfaceKinds: new Set(info.interfaceKinds),
    });
  }

  projection<View extends PrnsView>(
    view: View,
  ): PrnsProjection<PrnsProjectionValue<View>> {
    if (this.#projections === undefined) {
      throw new Error("DedicatedWorker projections are not ready");
    }
    return this.#projections.projection(view);
  }

  claimEvents(): StreamClaim<PrnsApplicationEvent> {
    const claim = this.#events.claim();
    if (claim.tag === "Claimed") {
      this.#sendEventRequest(Tag("ClaimApplicationEvents"));
    }
    return claim;
  }

  claimDiagnostics(): StreamClaim<PrnsDiagnosticEvent> {
    const claim = this.#diagnostics.claim();
    if (claim.tag === "Claimed") {
      this.#sendEventRequest(Tag("ClaimDiagnostics"));
    }
    return claim;
  }

  snapshot(): Promise<SnapshotOutcome> {
    if (this.#stoppedSnapshot !== undefined) {
      return Promise.resolve(this.#stoppedSnapshot);
    }
    return this.#call(workerCall("Snapshot"));
  }

  async hostSnapshot(): Promise<HostSnapshotOutcome> {
    const outcome = this.#stoppedHostSnapshot ??
      await this.#call(workerCall("HostSnapshot"));
    if (outcome.tag !== "Captured" || this.#persistenceFailureDetail === undefined) {
      return outcome;
    }
    return Tag("Captured", {
      ...outcome.data,
      persistence: {
        ...outcome.data.persistence,
        lastFailureDetail: this.#persistenceFailureDetail,
      },
    });
  }

  stop(): Promise<StopOutcome> {
    if (this.#stopCompleted) {
      return Promise.resolve(Tag("AlreadyStopped"));
    }
    if (this.#stopPromise !== undefined) {
      return this.#stopPromise;
    }
    this.#setLifecycle(Tag("Stopping"));
    this.#cancelPendingCommands();
    this.#stopPromise = this.#performStop();
    return this.#stopPromise;
  }

  webSocketConnect(
    url: string | URL,
    options: WebSocketConnectOptions,
  ): Promise<WebSocketConnectOutcome> {
    return this.#call(workerCall("WebSocketConnect", {
      url: url.toString(),
      options,
    })).then((outcome) => {
      if (outcome.tag !== "Connected") {
        return outcome;
      }
      const session = new WorkerWebSocketSession(this, outcome.data);
      this.#webSocketSessions.set(outcome.data.id, session);
      return Tag("Connected", session);
    });
  }

  async closeSession(id: number): Promise<InterfaceCloseOutcome> {
    const outcome = await this.#call(workerCall("InterfaceSessionClose", id));
    if (outcome.tag === "Closed") {
      this.#webSocketSessions.get(id)?.markClosed();
      this.#webSocketSessions.delete(id);
    }
    return outcome;
  }

  startAutoWifi(): Promise<AutoWifiControllerStatus> {
    return this.#call(workerCall("AutoWifiStart"));
  }

  autoWifiStatus(): Promise<AutoWifiControllerStatus> {
    return this.#call(workerCall("AutoWifiStatus"));
  }

  closeAutoWifi(): Promise<AutoWifiControllerCloseOutcome> {
    return this.#call(workerCall("AutoWifiClose"));
  }

  async #performStop(): Promise<StopOutcome> {
    this.#autoWifi.finishFromHostStop();
    const pageSessionFailures = await this.#closePageSessions();
    try {
      const stopped = await this.#requestWorkerShutdown();
      this.#stoppedSnapshot = stopped.snapshot;
      this.#stoppedHostSnapshot = stopped.hostSnapshot;
      for (const session of this.#webSocketSessions.values()) {
        session.markClosed();
      }
      this.#webSocketSessions.clear();
      const failures = [...pageSessionFailures];
      if (stopped.persistedState !== undefined && this.#persistenceStore !== undefined) {
        try {
          const saved = await this.#persistenceStore.save(
            parseBrowserPersistedState(stopped.persistedState),
          );
          if (saved.tag !== "Saved") {
            this.#persistenceFailureDetail = describePersistenceStoreFailure(saved);
            failures.push(this.#persistenceFailureDetail);
          }
        } catch (error) {
          this.#persistenceFailureDetail = describeHostError(error);
          failures.push(this.#persistenceFailureDetail);
        }
      }
      if (stopped.stopOutcome.tag === "OperationFailed") {
        failures.push(stopped.stopOutcome.data.detail);
      }
      this.#events.finish();
      this.#diagnostics.finish();
      if (failures.length > 0) {
        const detail = failures.join("; ");
        this.#setLifecycle(Tag("Failed", { cause: "BackendFailed", detail }));
        return Tag("OperationFailed", { operation: "stop", detail });
      }
      this.#setLifecycle(stopped.stopOutcome.tag === "Stopped"
        ? Tag("Stopped", { reason: "Requested" })
        : this.#lifecycle);
      return stopped.stopOutcome;
    } catch (error) {
      const detail = describeHostError(error);
      this.#failProtocol(detail);
      return Tag("OperationFailed", { operation: "stop", detail });
    } finally {
      this.#stopCompleted = true;
      this.#failPendingCalls("DedicatedWorker stopped");
      this.terminate();
    }
  }

  #requestWorkerShutdown(): Promise<WorkerShutdownState> {
    return new Promise((resolve, reject) => {
      this.#shutdownResolve = resolve;
      this.#shutdownReject = reject;
      this.#shutdownPort.postMessage(Tag("Stop"));
    });
  }

  #receiveShutdown(response: WorkerShutdownResponse): void {
    const resolve = this.#shutdownResolve;
    const reject = this.#shutdownReject;
    if (resolve === undefined || reject === undefined) {
      this.#failProtocol("DedicatedWorker sent an unexpected shutdown response");
      return;
    }
    if (response.tag === "ProtocolFailed") {
      const detail = workerProtocolDetail(response.data.detail);
      this.#shutdownResolve = undefined;
      this.#shutdownReject = undefined;
      reject(new Error(detail));
      return;
    }
    if (response.tag !== "Stopped") {
      throw new TypeError("DedicatedWorker sent an unknown shutdown response");
    }
    if (
      typeof response.data !== "object" ||
      response.data === null ||
      response.data.stopOutcome === undefined ||
      response.data.snapshot === undefined ||
      response.data.hostSnapshot === undefined
    ) {
      throw new TypeError("DedicatedWorker shutdown response is incomplete");
    }
    this.#shutdownResolve = undefined;
    this.#shutdownReject = undefined;
    resolve(response.data);
  }

  #cancelPendingCommands(): void {
    for (const [id, pending] of this.#pending) {
      if (
        pending.call.tag !== "Execute" &&
        pending.call.tag !== "SendResourceBlob"
      ) {
        continue;
      }
      this.#pending.delete(id);
      this.#ignoredSettlements.add(id);
      this.#releaseCallCapacity(pending.call);
      pending.settle(commandFailed(Tag("NodeStopped")));
    }
  }

  #releaseCallCapacity(call: WorkerCall): void {
    if (call.tag === "Execute" || call.tag === "SendResourceBlob") {
      this.#pendingCommandCount = Math.max(0, this.#pendingCommandCount - 1);
      return;
    }
    this.#pendingControlCount = Math.max(0, this.#pendingControlCount - 1);
  }

  async #closePageSessions(): Promise<string[]> {
    const sessions = [...this.#pageSessions.values()];
    this.#pageSessions.clear();
    const outcomes = await Promise.all(
      sessions.map(async (session): Promise<string | undefined> => {
        try {
          const closed = await session.close();
          return closed.tag === "Closed"
            ? undefined
            : describeInterfaceSessionFailure(closed);
        } catch (error) {
          return describeHostError(error);
        }
      }),
    );
    return outcomes.filter((outcome): outcome is string => outcome !== undefined);
  }

  #call<Call extends WorkerCall>(call: Call): Promise<WorkerCallOutcome<Call>> {
    if (this.#terminated) {
      return Promise.reject(new Error("DedicatedWorker has terminated"));
    }
    const command = call.tag === "Execute" || call.tag === "SendResourceBlob";
    if (command) {
      if (this.#pendingCommandCount >= this.#limits.pendingCommands) {
        return Promise.reject(new Error("DedicatedWorker command queue is full"));
      }
    } else if (this.#pendingControlCount >= MAXIMUM_PENDING_CONTROL_CALLS) {
      return Promise.reject(new Error("DedicatedWorker control queue is full"));
    }
    const id = this.#nextCallId;
    this.#nextCallId = this.#nextCallId === Number.MAX_SAFE_INTEGER ? 1 : this.#nextCallId + 1;
    return new Promise<WorkerCallOutcome<Call>>((settle, fail) => {
      this.#pending.set(id, {
        call,
        settle: settle as (outcome: unknown) => void,
        fail,
      });
      if (command) {
        this.#pendingCommandCount += 1;
      } else {
        this.#pendingControlCount += 1;
      }
      try {
        this.#controlSender.send({ id, call });
      } catch (error) {
        this.#pending.delete(id);
        this.#releaseCallCapacity(call);
        fail(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  #capabilityCall<Call extends WorkerCapabilityCall>(
    call: Call,
  ): Promise<WorkerCapabilityCallOutcome<Call>> {
    if (this.#terminated) {
      return Promise.reject(new Error("DedicatedWorker has terminated"));
    }
    if (this.#capabilityPending.size >= this.#limits.pendingCommands) {
      return Promise.reject(new Error("DedicatedWorker capability queue is full"));
    }
    const id = this.#nextCallId;
    this.#nextCallId = this.#nextCallId === Number.MAX_SAFE_INTEGER ? 1 : this.#nextCallId + 1;
    return new Promise<WorkerCapabilityCallOutcome<Call>>((settle, fail) => {
      this.#capabilityPending.set(id, {
        call,
        settle: settle as (outcome: unknown) => void,
        fail,
      });
      try {
        this.#capabilitySender.send({ id, call });
      } catch (error) {
        this.#capabilityPending.delete(id);
        fail(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  #receiveControl(message: WorkerControlResponse): void {
    match_into<void>().from(message, {
      Started: ({ outcome }) => {
        if (this.#startSettled || this.#startResolve === undefined) {
          this.#failProtocol("DedicatedWorker sent duplicate startup state");
          return;
        }
        this.#settleStart(workerStartOutcome(outcome));
      },
      Settlements: ({ batch }) => {
        this.#controlSettlements.receive(batch);
      },
      ProtocolFailed: ({ id, detail }) => {
        if (id !== undefined) {
          const pending = this.#pending.get(id);
          if (pending !== undefined) {
            this.#pending.delete(id);
            this.#releaseCallCapacity(pending.call);
            pending.fail(new Error(detail));
            return;
          }
        }
        this.#failProtocol(detail);
      },
      EventBackpressureExceeded: ({ rejectedEventBytes }) => {
        this.#failBackpressure(rejectedEventBytes);
      },
    });
  }

  #receiveWorkerMessage(
    message: unknown,
    receive: (message: { readonly tag: string; readonly data?: unknown }) => void,
  ): void {
    try {
      if (
        typeof message !== "object" ||
        message === null ||
        !("tag" in message) ||
        typeof message.tag !== "string"
      ) {
        throw new TypeError("DedicatedWorker message envelope is malformed");
      }
      receive(message as { readonly tag: string; readonly data?: unknown });
    } catch (error) {
      this.#failProtocol(describeHostError(error));
    }
  }

  #receiveControlSettlement(message: WorkerSettlement): void {
    if (this.#ignoredSettlements.delete(message.id)) {
      return;
    }
    const pending = this.#pending.get(message.id);
    if (pending === undefined) {
      this.#failProtocol(`DedicatedWorker settled unknown call ${message.id}`);
      return;
    }
    if (pending.call.tag !== message.call) {
      this.#failProtocol(`DedicatedWorker settled call ${message.id} as ${message.call}`);
      return;
    }
    this.#pending.delete(message.id);
    this.#releaseCallCapacity(pending.call);
    pending.settle(message.outcome);
  }

  #receiveCapability(message: WorkerCapabilityResponse): void {
    match_into<void>().from(message, {
      CapabilitySettlements: ({ batch }) => {
        this.#capabilitySettlements.receive(batch);
      },
      ProtocolFailed: ({ id, detail }) => {
        if (id !== undefined) {
          const pending = this.#capabilityPending.get(id);
          if (pending !== undefined) {
            this.#capabilityPending.delete(id);
            pending.fail(new Error(detail));
            return;
          }
        }
        this.#failProtocol(detail);
      },
    });
  }

  #receiveCapabilitySettlement(message: WorkerCapabilitySettlement): void {
    const pending = this.#capabilityPending.get(message.id);
    if (pending === undefined) {
      this.#failProtocol(`DedicatedWorker settled unknown capability call ${message.id}`);
      return;
    }
    if (pending.call.tag !== message.call) {
      this.#failProtocol(
        `DedicatedWorker settled capability call ${message.id} as ${message.call}`,
      );
      return;
    }
    this.#capabilityPending.delete(message.id);
    pending.settle(message.outcome);
  }

  #receiveProjection(message: WorkerProjectionMessage): void {
    if (message.tag === "ProjectionProtocolFailed") {
      this.#failProtocol(workerProtocolDetail(message.data.detail));
      return;
    }
    if (message.tag === "ProjectionSynchronized") {
      const id = workerMessageId(message.data.id, "projection");
      const pending = this.#pendingProjectionSynchronizations.get(id);
      if (pending === undefined) {
        this.#failProtocol(
          `DedicatedWorker synchronized unknown projection request ${id}`,
        );
        return;
      }
      const outcome = parseProjectionSynchronization(
        pending.view,
        message.data.outcome,
      );
      this.#pendingProjectionSynchronizations.delete(id);
      pending.settle(outcome);
      return;
    }
    if (message.tag !== "ProjectionBatch") {
      throw new TypeError("DedicatedWorker sent an unknown projection message");
    }
    const id = workerMessageId(message.data.id, "projection");
    const projections = this.#projections;
    if (projections === undefined) {
      this.#failProtocol("DedicatedWorker sent projections before startup completed");
      return;
    }
    try {
      this.#projectionUpdates.receive(message.data.batch);
      this.#sendProjectionRequest(
        Tag("AcknowledgeProjection", { id }),
      );
    } catch (error) {
      this.#failProtocol(describeHostError(error));
    }
  }

  #sendProjectionRequest(request: WorkerProjectionRequest): void {
    if (!this.#terminated) {
      this.#projectionsPort.postMessage(request);
    }
  }

  #synchronizeProjection(
    view: PrnsView,
  ): Promise<ProjectionSynchronization<unknown>> {
    if (this.#lifecycle.tag !== "Running") {
      return Promise.resolve(Tag("Unavailable", { lifecycle: this.#lifecycle }));
    }
    if (
      this.#pendingProjectionSynchronizations.size >=
        MAXIMUM_PENDING_PROJECTION_SYNCHRONIZATIONS
    ) {
      return Promise.resolve(Tag("Busy"));
    }
    const id = this.#nextProjectionSynchronizationId;
    this.#nextProjectionSynchronizationId = id === Number.MAX_SAFE_INTEGER
      ? 1
      : id + 1;
    return new Promise((resolve) => {
      this.#pendingProjectionSynchronizations.set(id, {
        view,
        settle: resolve,
      });
      this.#sendProjectionRequest(Tag("Synchronize", { id, view }));
    });
  }

  #applyProjectionUpdate(update: WorkerProjectionUpdate): void {
    const projections = this.#projections;
    if (projections === undefined) {
      throw new Error("DedicatedWorker projection store is unavailable");
    }
    match_into<void>().from(update, {
      Lifecycle: (snapshot) => {
        this.#lifecycle = snapshot.value;
        projections.replaceLifecycle(snapshot.value, snapshot.revision);
      },
      Interfaces: (snapshot) => projections.replaceInterfaces(
        snapshot.value,
        snapshot.revision,
      ),
      Routes: (snapshot) => projections.replaceRoutes(
        snapshot.value,
        snapshot.revision,
      ),
      Links: (snapshot) => projections.replaceLinks(
        snapshot.value,
        snapshot.revision,
      ),
      DiagnosticsReset: (snapshot) => projections.replaceDiagnostics(
        snapshot.value,
        snapshot.revision,
      ),
      DiagnosticsDelta: ({ revision, dropped, appended }) =>
        projections.appendDiagnostics(dropped, appended, revision),
    });
  }

  #receiveEvent(message: WorkerEventMessage): void {
    try {
      if (message.tag === "Batch") {
        const id = workerMessageId(message.data.id, "event");
        if (!(message.data.buffer instanceof ArrayBuffer)) {
          throw new TypeError("event batch buffer must be an ArrayBuffer");
        }
        for (const event of parseEventBatch(new Uint8Array(message.data.buffer))) {
          const outcome = match_into<LanePushOutcome | "Ignored">().from(event, {
            Application: (application) => this.#events.push(application),
            Diagnostic: (diagnostic) => this.#diagnostics.push(diagnostic),
            CommandResponse: ({ event: response }) => this.#events.push(response),
            CommandResponseSegment: ({ event: response }) => this.#events.push(response),
            CommandSettled: () => "Ignored",
          });
          if (outcome === "Rejected") {
            return;
          }
        }
        this.#acknowledgeEvent(id);
        return;
      }
      if (message.tag !== "Diagnostic") {
        throw new TypeError("DedicatedWorker sent an unknown event message");
      }
      const id = workerMessageId(message.data.id, "event");
      const diagnostic = parseWorkerDiagnosticEvent(message.data.event);
      if (this.#diagnostics.push(diagnostic) !== "Rejected") {
        this.#acknowledgeEvent(id);
      }
    } catch (error) {
      this.#failProtocol(describeHostError(error));
    }
  }

  #acknowledgeEvent(id: number): void {
    this.#sendEventRequest(Tag("Acknowledge", { id }));
  }

  #sendEventRequest(request: WorkerEventRequest): void {
    if (!this.#terminated) {
      this.#eventsPort.postMessage(request);
    }
  }

  #failBackpressure(rejectedEventBytes: number): void {
    if (!this.#startSettled && this.#startResolve !== undefined) {
      this.#settleStart(Tag("WorkerProtocolFailed", {
        detail: "DedicatedWorker application event backpressure exceeded during startup",
      }));
    }
    this.#setLifecycle(Tag("Failed", {
      cause: "EventBackpressureExceeded",
      limits: this.#limits,
      rejectedEventBytes,
    }));
    this.#events.finish();
    this.#diagnostics.finish();
    this.#failPendingCalls("application event backpressure exceeded");
    this.terminate();
  }

  #failProtocol(detail: string): void {
    if (!this.#startSettled && this.#startResolve !== undefined) {
      this.#settleStart(Tag("WorkerProtocolFailed", { detail }));
    }
    this.#setLifecycle(Tag("Failed", { cause: "ContractViolated", detail }));
    const error = new Error(detail);
    this.#failPendingCalls(detail);
    this.#events.fail(error);
    this.#diagnostics.fail(error);
    this.terminate();
  }

  #setLifecycle(lifecycle: LifecycleState): void {
    this.#lifecycle = lifecycle;
    this.#projections?.replaceLifecycle(lifecycle);
  }

  #failPendingCalls(detail: string): void {
    const error = new Error(detail);
    this.#shutdownReject?.(error);
    this.#shutdownResolve = undefined;
    this.#shutdownReject = undefined;
    for (const pending of this.#pending.values()) {
      if (
        pending.call.tag === "Execute" ||
        pending.call.tag === "SendResourceBlob"
      ) {
        pending.settle(commandFailed(Tag("WriteFailed", { detail })));
      } else {
        pending.fail(error);
      }
    }
    this.#pending.clear();
    this.#pendingCommandCount = 0;
    this.#pendingControlCount = 0;
    if (this.#lifecycle.tag !== "Running") {
      const unavailable = Tag("Unavailable", { lifecycle: this.#lifecycle });
      for (const pending of this.#pendingProjectionSynchronizations.values()) {
        pending.settle(unavailable);
      }
      this.#pendingProjectionSynchronizations.clear();
    }
    for (const pending of this.#capabilityPending.values()) {
      pending.fail(error);
    }
    this.#capabilityPending.clear();
  }

  #settleStart(
    outcome:
      | Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }>
      | ReturnType<typeof readyWorker>,
  ): void {
    const resolve = this.#startResolve;
    if (this.#startSettled || resolve === undefined) {
      return;
    }
    this.#startSettled = true;
    this.#startResolve = undefined;
    if (this.#startTimer !== undefined) {
      globalThis.clearTimeout(this.#startTimer);
      this.#startTimer = undefined;
    }
    resolve(outcome);
  }
}

class WorkerCapabilityHost implements BluetoothRuntimeHost, UsbAutoRuntimeHost {
  readonly #wasm: PrnsWasmModule;
  readonly #bleIdentityAvailability: BleIdentityAvailability;
  readonly #lifecycle: () => LifecycleState;
  readonly #call: <Call extends WorkerCapabilityCall>(
    call: Call,
  ) => Promise<WorkerCapabilityCallOutcome<Call>>;

  constructor(
    wasm: PrnsWasmModule,
    bleIdentityAvailability: BleIdentityAvailability,
    lifecycle: () => LifecycleState,
    call: <Call extends WorkerCapabilityCall>(
      call: Call,
    ) => Promise<WorkerCapabilityCallOutcome<Call>>,
  ) {
    this.#wasm = wasm;
    this.#bleIdentityAvailability = bleIdentityAvailability;
    this.#lifecycle = lifecycle;
    this.#call = call;
  }

  runtimeReadiness() {
    return this.#lifecycle().tag === "Running"
      ? Tag("Ready")
      : Tag("RuntimeRejected", {
          operation: "inspect-readiness" as const,
          detail: "DedicatedWorker runtime is not running",
        });
  }

  bluetoothIdentityReadiness() {
    return this.#bleIdentityAvailability.tag === "Available"
      ? Tag("Ready")
      : this.#bleIdentityAvailability;
  }

  bluetoothServiceUuid(): string {
    return this.#wasm.bluetoothServiceUuid();
  }

  bluetoothControlUuid(): string {
    return this.#wasm.bluetoothControlUuid();
  }

  bluetoothDataUuid(): string {
    return this.#wasm.bluetoothDataUuid();
  }

  bluetoothBitrateBps() {
    return bitrateBps(this.#wasm.bluetoothBitrateBps());
  }

  bluetoothHardwareMtu() {
    return hardwareMtu(this.#wasm.bluetoothHardwareMtu());
  }

  bluetoothDialerHello(): Uint8Array {
    return this.#bleIdentityAvailability.tag === "Available"
      ? this.#wasm.bluetoothDialerHello(this.#bleIdentityAvailability.data)
      : new Uint8Array();
  }

  bluetoothDecodeControl(bytes: Uint8Array): unknown {
    return this.#wasm.bluetoothDecodeControl(bytes);
  }

  bluetoothDataFragments(bytes: Uint8Array): Uint8Array[] {
    return this.#wasm.bluetoothDataFragments(packetFrame(bytes));
  }

  createBluetoothReassembler() {
    return new WorkerBluetoothReassembler(this.#call);
  }

  defaultUsbAutoFilters() {
    return [{
      vendorId: this.#wasm.usbAutoWebUsbVendorId(),
      productId: this.#wasm.usbAutoWebUsbProductId(),
    }];
  }

  usbAutoHostBitrateBps() {
    return bitrateBps(this.#wasm.usbAutoHostBitrateBps());
  }

  usbAutoHostHardwareMtu() {
    return hardwareMtu(this.#wasm.usbAutoHostHardwareMtu());
  }

  usbAutoNodeTagFor(interfaceId: InterfaceId): Uint8Array {
    return this.#wasm.usbAutoNodeTagFor(interfaceId);
  }

  usbAutoHostHelloFrame(): Uint8Array {
    return this.#wasm.usbAutoHostHelloFrame();
  }

  usbAutoHostHelloAckFrame(nodeTag: Uint8Array): Uint8Array {
    return this.#wasm.usbAutoHostHelloAckFrame(nodeTag);
  }

  usbAutoDataFrame(bytes: Uint8Array): Uint8Array {
    return this.#wasm.usbAutoDataFrame(packetFrame(bytes));
  }

  createUsbAutoDecoder() {
    return new WorkerUsbAutoDecoder(this.#call);
  }

  registerInterface(
    registration: Parameters<BluetoothRuntimeHost["registerInterface"]>[0],
  ): Promise<Awaited<ReturnType<BluetoothRuntimeHost["registerInterface"]>>>;
  registerInterface(
    registration: Parameters<UsbAutoRuntimeHost["registerInterface"]>[0],
  ): Promise<Awaited<ReturnType<UsbAutoRuntimeHost["registerInterface"]>>>;
  registerInterface(
    registration:
      | Parameters<BluetoothRuntimeHost["registerInterface"]>[0]
      | Parameters<UsbAutoRuntimeHost["registerInterface"]>[0],
  ): Promise<
    | Awaited<ReturnType<BluetoothRuntimeHost["registerInterface"]>>
    | Awaited<ReturnType<UsbAutoRuntimeHost["registerInterface"]>>
  > {
    return this.#call(workerCapabilityCall("RegisterInterface", registration));
  }

  deactivateInterface(
    interfaceId: InterfaceId,
  ): Promise<Awaited<ReturnType<BluetoothRuntimeHost["deactivateInterface"]>>> {
    return this.#call(workerCapabilityCall("DeactivateInterface", interfaceId));
  }

  ingest(
    interfaceId: InterfaceId,
    bytes: Parameters<BluetoothRuntimeHost["ingest"]>[1],
  ): Promise<Awaited<ReturnType<BluetoothRuntimeHost["ingest"]>>> {
    return this.#call(workerCapabilityCall("Ingest", { interfaceId, bytes }));
  }

  takeOutboundFor(
    interfaceId: InterfaceId,
    maximumFrames?: number,
  ): Promise<Awaited<ReturnType<BluetoothRuntimeHost["takeOutboundFor"]>>> {
    return this.#call(workerCapabilityCall("TakeOutbound", {
        interfaceId,
        ...(maximumFrames === undefined ? {} : { maximumFrames }),
    }));
  }

  waitForOutboundActivity(
    interfaceId: InterfaceId,
  ): ReturnType<BluetoothRuntimeHost["waitForOutboundActivity"]> {
    return this.#call(workerCapabilityCall("WaitForOutboundActivity", interfaceId));
  }
}

class WorkerBluetoothReassembler {
  readonly #call: <Call extends WorkerCapabilityCall>(
    call: Call,
  ) => Promise<WorkerCapabilityCallOutcome<Call>>;
  readonly #id: Promise<number>;
  #released = false;

  constructor(call: <Call extends WorkerCapabilityCall>(
    call: Call,
  ) => Promise<WorkerCapabilityCallOutcome<Call>>) {
    this.#call = call;
    this.#id = call(workerCapabilityCall("CreateBluetoothReassembler"));
  }

  async absorb(bytes: Uint8Array): Promise<Uint8Array | undefined> {
    if (this.#released) {
      return undefined;
    }
    return this.#call(workerCapabilityCall("AbsorbBluetoothFragment", {
      id: await this.#id,
      bytes,
    }));
  }

  release(): void {
    if (this.#released) {
      return;
    }
    this.#released = true;
    void this.#id.then((id) => this.#call(
      workerCapabilityCall("ReleaseBluetoothReassembler", id),
    )).catch(() => undefined);
  }
}

class WorkerUsbAutoDecoder {
  readonly #call: <Call extends WorkerCapabilityCall>(
    call: Call,
  ) => Promise<WorkerCapabilityCallOutcome<Call>>;
  readonly #id: Promise<number>;
  #released = false;

  constructor(call: <Call extends WorkerCapabilityCall>(
    call: Call,
  ) => Promise<WorkerCapabilityCallOutcome<Call>>) {
    this.#call = call;
    this.#id = call(workerCapabilityCall("CreateUsbAutoDecoder"));
  }

  async feed(bytes: Uint8Array): Promise<unknown[]> {
    if (this.#released) {
      return [];
    }
    return this.#call(workerCapabilityCall("FeedUsbAutoDecoder", {
      id: await this.#id,
      bytes,
    }));
  }

  release(): void {
    if (this.#released) {
      return;
    }
    this.#released = true;
    void this.#id.then((id) => this.#call(
      workerCapabilityCall("ReleaseUsbAutoDecoder", id),
    )).catch(() => undefined);
  }
}

class WorkerWebSocketInterface {
  readonly name = "websocket" as const;
  readonly #client: DedicatedWorkerPrns;

  constructor(client: DedicatedWorkerPrns) {
    this.#client = client;
  }

  connect(
    url: string | URL,
    options: WebSocketConnectOptions = {},
  ): Promise<WebSocketConnectOutcome> {
    return this.#client.webSocketConnect(url, options);
  }
}

class WorkerWebSocketSession implements WebSocketSession {
  readonly name = "websocket" as const;
  readonly interfaceId: InterfaceId;
  readonly url: string;
  readonly framing: WebSocketSession["framing"];
  readonly #client: DedicatedWorkerPrns;
  readonly #id: number;
  #status: InterfaceSessionStatus;

  constructor(client: DedicatedWorkerPrns, projection: WorkerSessionProjection) {
    this.#client = client;
    this.#id = projection.id;
    this.interfaceId = projection.interfaceId;
    this.url = projection.url;
    this.framing = projection.framing;
    this.#status = projection.status;
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  async close(): Promise<InterfaceCloseOutcome> {
    const outcome = await this.#client.closeSession(this.#id);
    if (outcome.tag === "Closed") {
      this.#status = Tag("Closed");
    }
    return outcome;
  }

  markClosed(): void {
    this.#status = Tag("Closed");
  }
}

class WorkerAutoWifiInterface {
  readonly name = "auto-wifi" as const;
  readonly #client: DedicatedWorkerPrns;
  #controller: WorkerAutoWifiController | undefined;

  constructor(client: DedicatedWorkerPrns) {
    this.#client = client;
  }

  start(): WorkerAutoWifiController {
    if (this.#controller !== undefined && !this.#controller.closed) {
      return this.#controller;
    }
    this.#controller = new WorkerAutoWifiController(this.#client);
    return this.#controller;
  }

  finishFromHostStop(): void {
    this.#controller?.finishFromHostStop();
  }
}

class WorkerAutoWifiController {
  readonly #client: DedicatedWorkerPrns;
  #status: AutoWifiControllerStatus = Tag("Starting");
  #closed = false;
  #timer: number;
  #refreshing: Promise<void> | undefined;

  constructor(client: DedicatedWorkerPrns) {
    this.#client = client;
    this.#refreshing = this.#start().finally(() => {
      this.#refreshing = undefined;
    });
    this.#timer = globalThis.setInterval(() => {
      this.#scheduleRefresh();
    }, AUTO_WIFI_STATUS_POLL_MILLIS);
  }

  get status(): AutoWifiControllerStatus {
    return this.#status;
  }

  get closed(): boolean {
    return this.#closed;
  }

  async close(): Promise<AutoWifiControllerCloseOutcome> {
    if (this.#closed) {
      return Tag("Closed");
    }
    this.#closed = true;
    globalThis.clearInterval(this.#timer);
    const outcome = await this.#client.closeAutoWifi();
    this.#status = Tag("Closed");
    return outcome;
  }

  finishFromHostStop(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    globalThis.clearInterval(this.#timer);
    this.#status = Tag("Closed");
  }

  async #refresh(): Promise<void> {
    if (this.#closed) {
      return;
    }
    try {
      const status = await this.#client.autoWifiStatus();
      if (!this.#closed) {
        this.#status = status;
      }
    } catch (error) {
      if (this.#closed) {
        return;
      }
      this.#status = Tag(
        "Unavailable",
        Tag("DiscoveryFailed", { detail: describeHostError(error) }),
      );
    }
  }

  #scheduleRefresh(): void {
    if (this.#closed || this.#refreshing !== undefined) {
      return;
    }
    this.#refreshing = this.#refresh().finally(() => {
      this.#refreshing = undefined;
    });
  }

  async #start(): Promise<void> {
    try {
      const status = await this.#client.startAutoWifi();
      if (!this.#closed) {
        this.#status = status;
      }
    } catch (error) {
      if (this.#closed) {
        return;
      }
      this.#status = Tag(
        "Unavailable",
        Tag("DiscoveryFailed", { detail: describeHostError(error) }),
      );
    }
  }
}

async function prepareWorker(
  options: DedicatedWorkerPrnsOptions,
): Promise<ReturnType<typeof Tag<"Prepared", WorkerPreparation>> | Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }>> {
  const loadedWasm = options.wasmModuleUrl === undefined
    ? await loadBundledWasm()
    : await loadWasmModule(options.wasmModuleUrl);
  if (loadedWasm.tag !== "Loaded") {
    return loadedWasm;
  }
  const identityOutcome = await prepareIdentity(options);
  if (identityOutcome.tag !== "PreparedIdentity") {
    return identityOutcome;
  }
  const bleIdentityAvailability = await loadOrCreateBleIdentity(
    options.bleIdentityStore ?? new BrowserLocalStorageBleIdentityStore(),
  );
  const persistence = await preparePersistence(options.persistenceStore);
  if (persistence.tag !== "PreparedPersistence") {
    return persistence;
  }
  const limits = browserLimits(options.limits ?? balancedLimits());
  const autoWifiSelectionSeed = await loadOrCreateAutoWifiSelectionSeed();
  return Tag("Prepared", {
    initialization: {
      identity: identityOutcome.data,
      ...(bleIdentityAvailability.tag === "Available"
        ? { bleIdentity: bleIdentityAvailability.data }
        : {}),
      ...(persistence.data === undefined ? {} : { persistedState: persistence.data }),
      persistenceEnabled: options.persistenceStore !== undefined,
      limits,
      ...(options.resourceCompressionModuleUrl === undefined
        ? {}
        : { resourceCompressionModuleUrl: options.resourceCompressionModuleUrl.href }),
      ...(options.wasmModuleUrl === undefined
        ? {}
        : { wasmModuleUrl: options.wasmModuleUrl.href }),
      ...(autoWifiSelectionSeed.tag === "Loaded"
        ? { autoWifiSelectionSeed: autoWifiSelectionSeed.data }
        : {}),
    },
    ...(options.persistenceStore === undefined
      ? {}
      : { persistenceStore: options.persistenceStore }),
    wasm: loadedWasm.data,
    bleIdentityAvailability,
  });
}

async function prepareIdentity(
  options: DedicatedWorkerPrnsOptions,
): Promise<ReturnType<typeof Tag<"PreparedIdentity", IdentitySecretKey>> | Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }>> {
  const store = options.identityStore;
  if (store !== undefined) {
    let loaded: IdentityLoadOutcome;
    try {
      loaded = await store.load(IDENTITY_SECRET_LENGTH);
    } catch (error) {
      return Tag("IdentityStoreFailed", {
        operation: "Load",
        detail: describeHostError(error),
      });
    }
    if (loaded.tag === "Loaded") {
      try {
        return Tag("PreparedIdentity", identitySecretKey(loaded.data, IDENTITY_SECRET_LENGTH));
      } catch (error) {
        return Tag("StoredIdentityInvalid", { detail: describeHostError(error) });
      }
    }
    if (loaded.tag !== "Missing") {
      return loaded;
    }
  }
  const generated = webCryptoIdentity(IDENTITY_SECRET_LENGTH);
  if (generated.tag !== "Generated") {
    return generated;
  }
  if (store !== undefined) {
    let saved: IdentitySaveOutcome;
    try {
      saved = await store.save(generated.data);
    } catch (error) {
      return Tag("IdentityStoreFailed", {
        operation: "Save",
        detail: describeHostError(error),
      });
    }
    if (saved.tag !== "Saved") {
      return saved;
    }
  }
  return Tag("PreparedIdentity", generated.data);
}

async function preparePersistence(
  store: BrowserPersistenceStore | undefined,
): Promise<ReturnType<typeof Tag<"PreparedPersistence", ReturnType<typeof parseBrowserPersistedState> | undefined>> | Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }>> {
  if (store === undefined) {
    return Tag("PreparedPersistence", undefined);
  }
  let loaded: PersistenceLoadOutcome;
  try {
    loaded = await store.load();
  } catch (error) {
    return Tag("PersistenceStoreFailed", {
      operation: "Load",
      detail: describeHostError(error),
    });
  }
  if (loaded.tag === "Missing") {
    return Tag("PreparedPersistence", undefined);
  }
  if (loaded.tag !== "Loaded") {
    return loaded;
  }
  try {
    return Tag("PreparedPersistence", parseBrowserPersistedState(loaded.data));
  } catch (error) {
    return Tag("StoredPersistenceInvalid", { detail: describeHostError(error) });
  }
}

function workerStartOutcome(
  value: unknown,
): Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }> | ReturnType<typeof readyWorker> {
  if (typeof value !== "object" || value === null || !("tag" in value)) {
    return Tag("WorkerProtocolFailed", {
      detail: "DedicatedWorker startup response is malformed",
    });
  }
  const outcome = value as { readonly tag: string; readonly data?: unknown };
  if (outcome.tag !== "Ready") {
    return outcome as Exclude<PrnsCreateOutcome, { readonly tag: "Ready" }>;
  }
  const data = outcome.data as {
    readonly backendInfo?: BackendInfo;
    readonly lifecycle?: LifecycleState;
    readonly hostSnapshot?: HostSnapshot;
  };
  if (
    data?.backendInfo === undefined ||
    data.lifecycle === undefined ||
    data.hostSnapshot === undefined
  ) {
    return Tag("WorkerProtocolFailed", {
      detail: "DedicatedWorker ready response is incomplete",
    });
  }
  return readyWorker({
    backendInfo: data.backendInfo,
    lifecycle: data.lifecycle,
    hostSnapshot: data.hostSnapshot,
  });
}

function readyWorker(state: WorkerReadyState) {
  return Tag("Ready", state);
}

function workerMessageId(raw: unknown, channel: string): number {
  if (typeof raw !== "number" || !Number.isSafeInteger(raw) || raw <= 0) {
    throw new TypeError(`${channel} message id must be a positive safe integer`);
  }
  return raw;
}

function workerProtocolDetail(raw: unknown): string {
  if (typeof raw !== "string" || raw.length === 0) {
    throw new TypeError("worker protocol failure detail must be a non-empty string");
  }
  return raw;
}

function workerCapabilityInvocationWireBytes(value: {
  readonly id: number;
  readonly call: WorkerCapabilityCall;
}): number {
  const call = value.call;
  if (
    call.tag === "Ingest" ||
    call.tag === "AbsorbBluetoothFragment" ||
    call.tag === "FeedUsbAutoDecoder"
  ) {
    return 64 + call.data.bytes.byteLength;
  }
  return 256;
}

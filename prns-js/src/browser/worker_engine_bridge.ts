import type { InterfaceId } from "../contract.js";
import type { PrnsDiagnosticEvent } from "./events.js";
import type { MainThreadPrnsOptions } from "./index.js";
import type { TransferredInterfaceOutboundOutcome } from "./outbound.js";
import type {
  WorkerCapabilityCall,
  WorkerCapabilityCallOutcome,
  WorkerSnapshotOutcome,
} from "./worker_protocol.js";

export type WorkerEngineHooks = {
  readonly eventBatchSink: (batch: Uint8Array) => void;
  readonly directDiagnosticSink: (event: PrnsDiagnosticEvent) => void;
  readonly autoWifiSelectionSeed?: Uint8Array;
};

type WorkerCapabilityDispatcher = <Call extends WorkerCapabilityCall>(
  call: Call,
) => Promise<WorkerCapabilityCallOutcome<Call>>;

const hooksByOptions = new WeakMap<MainThreadPrnsOptions, WorkerEngineHooks>();
const capabilityDispatchers = new WeakMap<object, WorkerCapabilityDispatcher>();
const snapshotCapturers = new WeakMap<object, () => WorkerSnapshotOutcome>();
const networkOutboundDispatchers = new WeakMap<
  object,
  (
    interfaceId: InterfaceId,
    maximumFrames?: number,
  ) => Promise<TransferredInterfaceOutboundOutcome>
>();

export function bindWorkerEngineOptions(
  options: MainThreadPrnsOptions,
  hooks: WorkerEngineHooks,
): MainThreadPrnsOptions {
  hooksByOptions.set(options, hooks);
  return options;
}

export function workerEngineHooks(
  options: MainThreadPrnsOptions,
): WorkerEngineHooks | undefined {
  return hooksByOptions.get(options);
}

export function registerWorkerCapabilityDispatcher(
  owner: object,
  dispatcher: WorkerCapabilityDispatcher,
): void {
  capabilityDispatchers.set(owner, dispatcher);
}

export function dispatchWorkerCapability<Call extends WorkerCapabilityCall>(
  owner: object,
  call: Call,
): Promise<WorkerCapabilityCallOutcome<Call>> {
  const dispatcher = capabilityDispatchers.get(owner);
  if (dispatcher === undefined) {
    return Promise.reject(new Error("worker capability dispatcher is unavailable"));
  }
  return dispatcher(call);
}

export function registerWorkerNetworkOutboundDispatcher(
  owner: object,
  dispatcher: (
    interfaceId: InterfaceId,
    maximumFrames?: number,
  ) => Promise<TransferredInterfaceOutboundOutcome>,
): void {
  networkOutboundDispatchers.set(owner, dispatcher);
}

export function dispatchWorkerNetworkOutbound(
  owner: object,
  interfaceId: InterfaceId,
  maximumFrames?: number,
): Promise<TransferredInterfaceOutboundOutcome> {
  const dispatcher = networkOutboundDispatchers.get(owner);
  if (dispatcher === undefined) {
    return Promise.reject(
      new Error("worker network outbound dispatcher is unavailable"),
    );
  }
  return dispatcher(interfaceId, maximumFrames);
}

export function registerWorkerSnapshotCapturer(
  owner: object,
  capture: () => WorkerSnapshotOutcome,
): void {
  snapshotCapturers.set(owner, capture);
}

export function captureWorkerSnapshot(owner: object): WorkerSnapshotOutcome {
  const capture = snapshotCapturers.get(owner);
  if (capture === undefined) {
    throw new Error("worker snapshot capturer is unavailable");
  }
  return capture();
}

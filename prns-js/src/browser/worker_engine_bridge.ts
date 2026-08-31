import type { PrnsDiagnosticEvent } from "./events.js";
import type { MainThreadPrnsOptions } from "./index.js";
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

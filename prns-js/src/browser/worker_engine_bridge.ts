import type { PrnsDiagnosticEvent } from "./events.js";
import type { MainThreadPrnsOptions } from "./index.js";
import type { WorkerCapabilityCall } from "./worker_protocol.js";

export type WorkerEngineHooks = {
  readonly eventBatchSink: (batch: Uint8Array) => void;
  readonly directDiagnosticSink: (event: PrnsDiagnosticEvent) => void;
  readonly autoWifiSelectionSeed?: Uint8Array;
};

type WorkerCapabilityDispatcher = (
  call: WorkerCapabilityCall,
) => Promise<unknown>;

const hooksByOptions = new WeakMap<MainThreadPrnsOptions, WorkerEngineHooks>();
const capabilityDispatchers = new WeakMap<object, WorkerCapabilityDispatcher>();

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

export function dispatchWorkerCapability(
  owner: object,
  call: WorkerCapabilityCall,
): Promise<unknown> {
  const dispatcher = capabilityDispatchers.get(owner);
  if (dispatcher === undefined) {
    return Promise.reject(new Error("worker capability dispatcher is unavailable"));
  }
  return dispatcher(call);
}

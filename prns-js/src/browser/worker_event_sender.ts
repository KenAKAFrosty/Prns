import { Tag } from "../casework.js";
import type { PrnsLimits } from "../contract.js";
import {
  retainApplicationEventBatchProjection,
  summarizeEventBatchProjection,
} from "../event_projection.js";
import type { EventBatchProjectionSummary } from "../event_projection.js";
import type { PrnsDiagnosticEvent } from "./events.js";
import type {
  WorkerEventAcknowledgement,
  WorkerEventMessage,
} from "./worker_protocol.js";

type QueuedWorkerEvent = {
  readonly message: WorkerEventMessage;
  readonly applicationEvents: number;
  readonly diagnostics: number;
  readonly retainedEventBytes: number;
};

type WorkerEventSenderFailure = {
  readonly protocol: (detail: string) => void;
  readonly backpressure: (rejectedEventBytes: number) => void;
};

export class BoundedWorkerEventSender {
  readonly #port: MessagePort;
  readonly #limits: PrnsLimits;
  readonly #failure: WorkerEventSenderFailure;
  readonly #queued: QueuedWorkerEvent[] = [];
  #inFlight: QueuedWorkerEvent | undefined;
  #applicationEvents = 0;
  #diagnostics = 0;
  #retainedEventBytes = 0;
  #droppedDiagnostics = 0n;
  #nextId = 1;
  #failed = false;

  constructor(
    port: MessagePort,
    limits: PrnsLimits,
    failure: WorkerEventSenderFailure,
  ) {
    this.#port = port;
    this.#limits = limits;
    this.#failure = failure;
    port.addEventListener(
      "message",
      (event: MessageEvent<WorkerEventAcknowledgement>) => {
        this.#receiveAcknowledgement(event.data);
      },
    );
  }

  sendBatch(batch: Uint8Array): void {
    this.#requireActive();
    let summary: EventBatchProjectionSummary;
    try {
      summary = summarizeEventBatchProjection(batch);
    } catch (error) {
      this.#failProtocol(error instanceof Error ? error.message : String(error));
      throw error;
    }
    let transferable = batch;
    if (this.#diagnostics + summary.diagnostics > this.#limits.diagnostics) {
      this.#droppedDiagnostics += BigInt(summary.diagnostics);
      transferable = retainApplicationEventBatchProjection(batch);
      summary = {
        applicationEvents: summary.applicationEvents,
        diagnostics: 0,
        retainedEventBytes: summary.retainedEventBytes,
      };
    }
    this.#flushDiagnosticGap();
    if (summary.applicationEvents === 0 && summary.diagnostics === 0) {
      return;
    }
    const buffer = transferableBuffer(transferable);
    this.#enqueue({
      message: { type: "batch", id: this.#mintId(), buffer },
      ...summary,
    });
  }

  sendDiagnostic(event: PrnsDiagnosticEvent): void {
    this.#requireActive();
    this.#flushDiagnosticGap();
    if (this.#diagnostics === this.#limits.diagnostics) {
      this.#droppedDiagnostics += 1n;
      return;
    }
    this.#enqueue({
      message: { type: "diagnostic", id: this.#mintId(), event },
      applicationEvents: 0,
      diagnostics: 1,
      retainedEventBytes: 0,
    });
  }

  #enqueue(queued: QueuedWorkerEvent): void {
    if (
      this.#applicationEvents + queued.applicationEvents >
        this.#limits.applicationEvents ||
      this.#retainedEventBytes + queued.retainedEventBytes >
        this.#limits.retainedEventBytes
    ) {
      this.#failBackpressure(queued.retainedEventBytes);
      throw new Error("DedicatedWorker application event queue exceeded its bounds");
    }
    this.#applicationEvents += queued.applicationEvents;
    this.#diagnostics += queued.diagnostics;
    this.#retainedEventBytes += queued.retainedEventBytes;
    this.#queued.push(queued);
    this.#dispatch();
  }

  #dispatch(): void {
    if (this.#failed || this.#inFlight !== undefined) {
      return;
    }
    const next = this.#queued.shift();
    if (next === undefined) {
      return;
    }
    this.#inFlight = next;
    if (next.message.type === "batch") {
      this.#port.postMessage(next.message, [next.message.buffer]);
      return;
    }
    this.#port.postMessage(next.message);
  }

  #receiveAcknowledgement(message: WorkerEventAcknowledgement): void {
    if (this.#failed) {
      return;
    }
    const inFlight = this.#inFlight;
    if (
      message?.type !== "acknowledge" ||
      !Number.isSafeInteger(message.id) ||
      inFlight === undefined ||
      message.id !== inFlight.message.id
    ) {
      this.#failProtocol("DedicatedWorker event channel received an invalid acknowledgement");
      return;
    }
    this.#inFlight = undefined;
    this.#applicationEvents -= inFlight.applicationEvents;
    this.#diagnostics -= inFlight.diagnostics;
    this.#retainedEventBytes -= inFlight.retainedEventBytes;
    this.#flushDiagnosticGap();
    this.#dispatch();
  }

  #flushDiagnosticGap(): void {
    if (
      this.#droppedDiagnostics === 0n ||
      this.#diagnostics === this.#limits.diagnostics
    ) {
      return;
    }
    const count = this.#droppedDiagnostics;
    this.#droppedDiagnostics = 0n;
    this.#enqueue({
      message: {
        type: "diagnostic",
        id: this.#mintId(),
        event: Tag("DiagnosticsDropped", { count }),
      },
      applicationEvents: 0,
      diagnostics: 1,
      retainedEventBytes: 0,
    });
  }

  #mintId(): number {
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    return id;
  }

  #requireActive(): void {
    if (this.#failed) {
      throw new Error("DedicatedWorker event sender has failed");
    }
  }

  #failBackpressure(rejectedEventBytes: number): void {
    if (this.#failed) {
      return;
    }
    this.#failed = true;
    this.#queued.length = 0;
    this.#failure.backpressure(rejectedEventBytes);
  }

  #failProtocol(detail: string): void {
    if (this.#failed) {
      return;
    }
    this.#failed = true;
    this.#queued.length = 0;
    this.#failure.protocol(detail);
  }
}

function transferableBuffer(bytes: Uint8Array): ArrayBuffer {
  if (
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength &&
    bytes.buffer instanceof ArrayBuffer
  ) {
    return bytes.buffer;
  }
  return bytes.slice().buffer;
}

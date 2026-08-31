import { Tag } from "../casework.js";
import type {
  WorkerProjectionMessage,
  WorkerProjectionRequest,
  WorkerProjectionUpdate,
} from "./worker_protocol.js";
import { WireBatchEncoder } from "../worker_wire/wire_batch.js";

export class WorkerProjectionSender {
  readonly #port: MessagePort;
  readonly #failed: (detail: string) => void;
  readonly #pending = new Map<WorkerProjectionUpdate["tag"], WorkerProjectionUpdate>();
  readonly #encoder = new WireBatchEncoder<WorkerProjectionUpdate>();
  #inFlight: number | undefined;
  #nextId = 1;
  #scheduled = false;
  #closed = false;

  constructor(port: MessagePort, failed: (detail: string) => void) {
    this.#port = port;
    this.#failed = failed;
  }

  send(update: WorkerProjectionUpdate): void {
    if (this.#closed) {
      return;
    }
    this.#queue(update);
    if (this.#inFlight === undefined && !this.#scheduled) {
      this.#scheduled = true;
      queueMicrotask(() => this.#flush());
    }
  }

  #queue(update: WorkerProjectionUpdate): void {
    if (update.tag === "DiagnosticsReset") {
      this.#pending.delete("DiagnosticsDelta");
      this.#pending.set(update.tag, update);
      return;
    }
    if (update.tag !== "DiagnosticsDelta") {
      this.#pending.set(update.tag, update);
      return;
    }
    const reset = this.#pending.get("DiagnosticsReset");
    if (reset?.tag === "DiagnosticsReset") {
      const value = [
        ...reset.data.value.slice(update.data.dropped),
        ...update.data.appended,
      ];
      this.#pending.set(
        "DiagnosticsReset",
        Tag("DiagnosticsReset", {
          revision: update.data.revision,
          value,
        }),
      );
      return;
    }
    const previous = this.#pending.get("DiagnosticsDelta");
    if (previous?.tag !== "DiagnosticsDelta") {
      this.#pending.set(update.tag, update);
      return;
    }
    const consumedAppended = Math.min(
      previous.data.appended.length,
      update.data.dropped,
    );
    const dropped = previous.data.dropped +
      update.data.dropped - consumedAppended;
    const appended = [
      ...previous.data.appended.slice(consumedAppended),
      ...update.data.appended,
    ];
    this.#pending.set(
      "DiagnosticsDelta",
      Tag("DiagnosticsDelta", {
        revision: update.data.revision,
        dropped,
        appended,
      }),
    );
  }

  receive(request: WorkerProjectionRequest): void {
    if (request.tag !== "AcknowledgeProjection") {
      this.#fail("projection sender received an unexpected request");
      return;
    }
    if (
      this.#inFlight === undefined ||
      !Number.isSafeInteger(request.data.id) ||
      request.data.id !== this.#inFlight
    ) {
      this.#fail("projection sender received an invalid acknowledgement");
      return;
    }
    this.#inFlight = undefined;
    if (this.#pending.size > 0 && !this.#scheduled) {
      this.#scheduled = true;
      queueMicrotask(() => this.#flush());
    }
  }

  close(): void {
    this.#closed = true;
    this.#pending.clear();
  }

  #flush(): void {
    this.#scheduled = false;
    if (this.#closed || this.#inFlight !== undefined || this.#pending.size === 0) {
      return;
    }
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    const updates = [...this.#pending.values()];
    this.#pending.clear();
    this.#inFlight = id;
    try {
      const encoded = this.#encoder.encode(updates);
      const message: WorkerProjectionMessage = Tag("ProjectionBatch", {
        id,
        batch: encoded.message,
      });
      this.#port.postMessage(message, [...encoded.transfer]);
    } catch (error) {
      this.#fail(error instanceof Error ? error.message : String(error));
    }
  }

  #fail(detail: string): void {
    if (this.#closed) {
      return;
    }
    this.close();
    this.#failed(detail);
  }
}

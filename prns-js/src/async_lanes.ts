import { Tag } from "./casework.js";
import type { Tag as Tagged } from "./casework.js";

export type AsyncLaneName =
  | "ApplicationEvents"
  | "Diagnostics"
  | "Resource";

export type StreamClaim<Value> =
  | Tagged<"Claimed", AsyncIterableIterator<Value>>
  | Tagged<"AlreadyClaimed", { readonly lane: AsyncLaneName }>;

export type LanePushOutcome = "Queued" | "Delivered" | "Dropped" | "Rejected";

export type AsyncLaneOptions<Value> = {
  readonly name: AsyncLaneName;
  readonly maximumValues: number;
  readonly maximumBytes: number;
  readonly measure: (value: Value) => number;
  readonly onRejected?: (rejectedBytes: number) => void;
  readonly gap?: (droppedNewest: bigint) => Value;
  readonly onBeforeNext?: () => void;
};

export class BoundedAsyncLane<Value> {
  readonly #options: AsyncLaneOptions<Value>;
  readonly #queued: Array<{ readonly value: Value; readonly bytes: number }> =
    [];
  #retainedBytes = 0;
  #droppedNewest = 0n;
  #waiting:
    | {
        readonly resolve: (result: IteratorResult<Value>) => void;
        readonly reject: (error: unknown) => void;
      }
    | undefined;
  #claimed = false;
  #finished = false;
  #failure: unknown;
  #tail: Promise<void> = Promise.resolve();

  constructor(options: AsyncLaneOptions<Value>) {
    this.#options = options;
  }

  push(value: Value): LanePushOutcome {
    if (this.#finished || this.#failure !== undefined) {
      return "Rejected";
    }
    if (this.#waiting) {
      const waiting = this.#waiting;
      this.#waiting = undefined;
      waiting.resolve({ done: false, value });
      return "Delivered";
    }
    const retained = this.#options.measure(value);
    if (
      this.#queued.length >= this.#options.maximumValues ||
      this.#retainedBytes + retained > this.#options.maximumBytes
    ) {
      if (this.#options.gap) {
        this.#droppedNewest += 1n;
        return "Dropped";
      }
      this.#options.onRejected?.(retained);
      return "Rejected";
    }
    this.#queued.push({ value, bytes: retained });
    this.#retainedBytes += retained;
    return "Queued";
  }

  finish(): void {
    this.#finished = true;
    if (this.#waiting) {
      const waiting = this.#waiting;
      this.#waiting = undefined;
      waiting.resolve({ done: true, value: undefined });
    }
  }

  fail(error: unknown): void {
    this.#failure = error;
    if (this.#waiting) {
      const waiting = this.#waiting;
      this.#waiting = undefined;
      waiting.reject(error);
    }
  }

  claim(): StreamClaim<Value> {
    if (this.#claimed) {
      return Tag("AlreadyClaimed", { lane: this.#options.name });
    }
    this.#claimed = true;
    const iterator: AsyncIterableIterator<Value> = {
      next: () => this.#serializedNext(),
      [Symbol.asyncIterator]: () => iterator,
    };
    return Tag("Claimed", iterator);
  }

  #serializedNext(): Promise<IteratorResult<Value>> {
    const next = this.#tail.then(() => this.#take());
    this.#tail = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }

  #take(): Promise<IteratorResult<Value>> {
    this.#options.onBeforeNext?.();
    const queued = this.#queued.shift();
    if (queued) {
      this.#retainedBytes -= queued.bytes;
      return Promise.resolve({ done: false, value: queued.value });
    }
    if (this.#droppedNewest > 0n && this.#options.gap) {
      const count = this.#droppedNewest;
      this.#droppedNewest = 0n;
      return Promise.resolve({
        done: false,
        value: this.#options.gap(count),
      });
    }
    if (this.#failure !== undefined) {
      return Promise.reject(this.#failure);
    }
    if (this.#finished) {
      return Promise.resolve({ done: true, value: undefined });
    }
    return new Promise((resolve, reject) => {
      this.#waiting = { resolve, reject };
    });
  }
}

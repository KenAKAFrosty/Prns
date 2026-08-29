import {
  MAXIMUM_WIRE_BATCH_ITEMS,
  WireBatchDecoder,
  WireBatchEncoder,
} from "./wire_batch.js";
import type { WireBatch } from "./wire_batch.js";
import type { WireCodec, WireCodecPolicy } from "./wire_batch.js";

export type WirePort = {
  postMessage(message: unknown, transfer?: Transferable[]): void;
};

export type TaskScheduler = (task: () => void) => void;

export type BatchedPortOptions<Value> = {
  readonly port: WirePort;
  readonly wrap: (batch: WireBatch) => unknown;
  readonly maximumItems: number;
  readonly maximumQueuedItems: number;
  readonly maximumBytes: number;
  readonly measureBytes: (value: Value) => number;
  readonly scheduleTask: TaskScheduler;
  readonly failed: (error: unknown) => void;
  readonly codecPolicy?: WireCodecPolicy;
  readonly codec?: WireCodec<Value>;
};

export class BatchedPortSender<Value> {
  readonly #options: BatchedPortOptions<Value>;
  readonly #encoder: WireBatchEncoder<Value>;
  readonly #queued: Value[] = [];
  #scheduled = false;
  #failed = false;

  constructor(options: BatchedPortOptions<Value>) {
    if (
      !Number.isSafeInteger(options.maximumItems) ||
      options.maximumItems <= 0 ||
      options.maximumItems > MAXIMUM_WIRE_BATCH_ITEMS
    ) {
      throw new TypeError(
        `batched port item quantum must be between 1 and ${MAXIMUM_WIRE_BATCH_ITEMS}`,
      );
    }
    if (!Number.isSafeInteger(options.maximumBytes) || options.maximumBytes <= 0) {
      throw new TypeError("batched port byte quantum must be a positive integer");
    }
    if (
      !Number.isSafeInteger(options.maximumQueuedItems) ||
      options.maximumQueuedItems < options.maximumItems
    ) {
      throw new TypeError("batched port queue bound must contain at least one item quantum");
    }
    this.#options = options;
    this.#encoder = new WireBatchEncoder({
      ...options.codecPolicy,
      ...(options.codec === undefined ? {} : { codec: options.codec }),
    });
  }

  send(value: Value): void {
    if (this.#failed) {
      throw new Error("batched port sender has failed");
    }
    if (this.#queued.length >= this.#options.maximumQueuedItems) {
      throw new Error("batched port sender queue is full");
    }
    this.#queued.push(value);
    if (!this.#scheduled) {
      this.#scheduled = true;
      queueMicrotask(() => this.#flush());
    }
  }

  fail(): void {
    this.#failed = true;
    this.#queued.length = 0;
  }

  #flush(): void {
    if (this.#failed) {
      return;
    }
    this.#scheduled = false;
    try {
      const grain: Value[] = [];
      let bytes = 0;
      while (grain.length < this.#options.maximumItems) {
        if (grain.length >= this.#queued.length) {
          break;
        }
        const value = this.#queued[grain.length] as Value;
        const valueBytes = this.#options.measureBytes(value);
        if (!Number.isSafeInteger(valueBytes) || valueBytes < 0) {
          throw new TypeError("batched port byte measure must be a non-negative safe integer");
        }
        if (grain.length > 0 && bytes + valueBytes > this.#options.maximumBytes) {
          break;
        }
        grain.push(value);
        bytes += valueBytes;
      }
      this.#queued.splice(0, grain.length);
      const encoded = this.#encoder.encode(grain);
      this.#options.port.postMessage(
        this.#options.wrap(encoded.message),
        [...encoded.transfer],
      );
    } catch (error) {
      this.#failed = true;
      this.#queued.length = 0;
      this.#options.failed(error);
      return;
    }
    if (this.#queued.length > 0) {
      this.#scheduled = true;
      this.#options.scheduleTask(() => this.#flush());
    }
  }
}

export class BatchedPortReceiver<Value> {
  readonly #decoder: WireBatchDecoder<Value>;
  readonly #receive: (value: Value) => void;

  constructor(
    receive: (value: Value) => void,
    codecs: readonly WireCodec<Value>[] = [],
  ) {
    this.#receive = receive;
    this.#decoder = new WireBatchDecoder(codecs);
  }

  receive(batch: WireBatch): void {
    for (const value of this.#decoder.decode(batch)) {
      this.#receive(value as Value);
    }
  }
}

export function messageTaskScheduler(): TaskScheduler {
  const channel = new MessageChannel();
  const tasks: (() => void)[] = [];
  channel.port1.addEventListener("message", () => {
    tasks.shift()?.();
  });
  channel.port1.start();
  return (task) => {
    tasks.push(task);
    channel.port2.postMessage(undefined);
  };
}

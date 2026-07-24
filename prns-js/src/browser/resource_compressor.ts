import { Tag, match } from "../casework.js";

type CompressionRequest = {
  readonly id: number;
  readonly moduleUrl: string;
  readonly payload: ArrayBuffer;
  readonly packedMetadata?: ArrayBuffer;
};

type CompressionResponse = {
  readonly id: number;
  readonly candidate?: ArrayBuffer;
  readonly error?: string;
};

type CompressorState =
  | Tag<"Idle">
  | Tag<"Running", { readonly worker: Worker }>
  | Tag<"Unavailable">;

const RESOURCE_COMPRESSION_TIMEOUT_MS = 5_000;

export class BrowserResourceCompressor {
  #state: CompressorState = Tag("Idle");
  #nextRequestId = 0;
  #pending = new Map<
    number,
    {
      readonly settle: (candidate: Uint8Array | undefined) => void;
      readonly timeout: number;
    }
  >();

  compress(
    payload: Uint8Array,
    packedMetadata: Uint8Array | undefined,
    moduleUrl: string,
  ): Promise<Uint8Array | undefined> {
    const worker = this.#worker();
    if (worker === undefined) {
      return Promise.resolve(undefined);
    }
    const id = this.#nextRequestId;
    this.#nextRequestId += 1;
    const payloadBuffer = transferableCopy(payload);
    const packedMetadataBuffer =
      packedMetadata === undefined
        ? undefined
        : transferableCopy(packedMetadata);
    return new Promise((settle) => {
      const timeout = globalThis.setTimeout(() => {
        const pending = this.#pending.get(id);
        if (pending === undefined) {
          return;
        }
        this.#pending.delete(id);
        pending.settle(undefined);
        this.#disable();
      }, RESOURCE_COMPRESSION_TIMEOUT_MS);
      this.#pending.set(id, { settle, timeout });
      try {
        const request: CompressionRequest =
          packedMetadataBuffer === undefined
            ? {
                id,
                moduleUrl,
                payload: payloadBuffer,
              }
            : {
                id,
                moduleUrl,
                payload: payloadBuffer,
                packedMetadata: packedMetadataBuffer,
              };
        const transfers =
          packedMetadataBuffer === undefined
            ? [payloadBuffer]
            : [payloadBuffer, packedMetadataBuffer];
        worker.postMessage(request, transfers);
      } catch {
        const pending = this.#pending.get(id);
        if (pending !== undefined) {
          globalThis.clearTimeout(pending.timeout);
        }
        this.#pending.delete(id);
        settle(undefined);
        this.#disable();
      }
    });
  }

  #worker(): Worker | undefined {
    return match(this.#state, {
      Idle: () => {
        if (typeof Worker !== "function") {
          this.#state = Tag("Unavailable");
          return undefined;
        }
        try {
          const worker = new Worker(
            new URL("./resource_compression_worker.js", import.meta.url),
            { type: "module" },
          );
          worker.addEventListener("message", (event: MessageEvent<unknown>) => {
            this.#receive(event.data);
          });
          worker.addEventListener("error", () => {
            this.#disable();
          });
          this.#state = Tag("Running", { worker });
          return worker;
        } catch {
          this.#state = Tag("Unavailable");
          return undefined;
        }
      },
      Running: ({ worker }) => worker,
      Unavailable: () => undefined,
    });
  }

  #receive(raw: unknown): void {
    if (
      typeof raw !== "object" ||
      raw === null ||
      !("id" in raw) ||
      typeof raw.id !== "number"
    ) {
      this.#disable();
      return;
    }
    const response = raw as CompressionResponse;
    const pending = this.#pending.get(response.id);
    if (pending === undefined) {
      return;
    }
    this.#pending.delete(response.id);
    globalThis.clearTimeout(pending.timeout);
    if (response.error !== undefined) {
      pending.settle(undefined);
      this.#disable();
      return;
    }
    if (response.candidate === undefined) {
      pending.settle(undefined);
      return;
    }
    if (!(response.candidate instanceof ArrayBuffer)) {
      pending.settle(undefined);
      this.#disable();
      return;
    }
    pending.settle(new Uint8Array(response.candidate));
  }

  #disable(): void {
    match(this.#state, {
      Idle: () => undefined,
      Running: ({ worker }) => worker.terminate(),
      Unavailable: () => undefined,
    });
    this.#state = Tag("Unavailable");
    for (const pending of this.#pending.values()) {
      globalThis.clearTimeout(pending.timeout);
      pending.settle(undefined);
    }
    this.#pending.clear();
  }
}

function transferableCopy(bytes: Uint8Array): ArrayBuffer {
  const copy = bytes.slice();
  return copy.buffer as ArrayBuffer;
}

export const browserResourceCompressor =
  new BrowserResourceCompressor();

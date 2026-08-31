import { Tag, match, match_into } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import {
  WebCryptoResourceDigester,
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
} from "./resource_crypto.js";
import type {
  ResourceOpenCryptoJob,
  ResourceSealCryptoJob,
} from "./resource_crypto.js";
import type {
  ResourceCryptoWorkerRequest,
  ResourceCryptoWorkerResponse,
} from "./resource_crypto_worker_protocol.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type ResourceCryptoPoolFailure =
  | Tagged<"Busy">
  | Tagged<"Unavailable">
  | Tagged<"Failed", { readonly detail: string }>;

export type ResourceCryptoSealSettlement =
  | Tagged<
      "Sealed",
      {
        readonly sealed: OwnedBytes;
        readonly plaintext: OwnedBytes;
      }
    >
  | ResourceCryptoPoolFailure;

export type ResourceCryptoOpenSettlement =
  | Tagged<"Opened", OwnedBytes>
  | Tagged<"Refused">
  | ResourceCryptoPoolFailure;

export type ResourceCryptoDigestSettlement =
  | Tagged<
      "Digested",
      {
        readonly plaintext: OwnedBytes;
        readonly hash: OwnedBytes;
        readonly proof: OwnedBytes;
      }
    >
  | ResourceCryptoPoolFailure;

export type ResourceCryptoPoolReadiness =
  | Tagged<"Ready", { readonly workers: number }>
  | Tagged<"Unavailable">;

export type ResourceCryptoSealOutcome = Extract<
  ResourceCryptoSealSettlement,
  { readonly tag: "Sealed" | "Busy" | "Failed" }
>;

export type ResourceCryptoOpenOutcome = Extract<
  ResourceCryptoOpenSettlement,
  { readonly tag: "Opened" | "Refused" | "Busy" | "Failed" }
>;

export type ResourceCryptoDigestOutcome = Extract<
  ResourceCryptoDigestSettlement,
  { readonly tag: "Digested" | "Busy" | "Failed" }
>;

type ResourceCryptoSettlement =
  | ResourceCryptoSealSettlement
  | ResourceCryptoOpenSettlement
  | ResourceCryptoDigestSettlement;

type QueuedJob =
  | Tagged<
      "Seal",
      {
        readonly id: number;
        readonly request: Extract<ResourceCryptoWorkerRequest, { readonly tag: "Seal" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: ResourceCryptoSettlement) => void;
      }
    >
  | Tagged<
      "Open",
      {
        readonly id: number;
        readonly request: Extract<ResourceCryptoWorkerRequest, { readonly tag: "Open" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: ResourceCryptoSettlement) => void;
      }
    >
  | Tagged<
      "Digest",
      {
        readonly id: number;
        readonly request: Extract<ResourceCryptoWorkerRequest, { readonly tag: "Digest" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: ResourceCryptoSettlement) => void;
      }
    >;

type WorkerState =
  | Tagged<"Starting">
  | Tagged<"Ready">
  | Tagged<"Running", QueuedJob>
  | Tagged<"Failed">;

type WorkerSlot = {
  readonly worker: Worker;
  readonly startupTimeout: number;
  state: WorkerState;
};

const MAXIMUM_CRYPTO_WORKERS = 16;
const MAXIMUM_PENDING_CRYPTO_JOBS = 16;
const MAXIMUM_PENDING_CRYPTO_BYTES = 64 * 1_024 * 1_024;
const RESOURCE_CRYPTO_WORKER_START_TIMEOUT_MILLIS = 5_000;

export class BrowserResourceCryptoExecutor {
  readonly #pool: WebCryptoResourceWorkerPool;
  readonly #sealer = new WebCryptoResourceSealer();
  readonly #opener = new WebCryptoResourceOpener();
  readonly #digester = new WebCryptoResourceDigester();
  #closed = false;

  constructor(workers: number) {
    this.#pool = new WebCryptoResourceWorkerPool(workers);
  }

  async seal(job: ResourceSealCryptoJob): Promise<ResourceCryptoSealOutcome> {
    if (this.#closed) {
      return closedCryptoExecutor();
    }
    const outcome = await this.#pool.donateSeal(job);
    return match(outcome, {
      Sealed: (data) => Tag("Sealed", data),
      Busy: () => Tag("Busy"),
      Unavailable: () => this.#closed ? closedCryptoExecutor() : this.#sealInline(job),
      Failed: (data) => Tag("Failed", data),
    });
  }

  async open(job: ResourceOpenCryptoJob): Promise<ResourceCryptoOpenOutcome> {
    if (this.#closed) {
      return closedCryptoExecutor();
    }
    const outcome = await this.#pool.donateOpen(job);
    return match(outcome, {
      Opened: (plaintext) => Tag("Opened", plaintext),
      Refused: () => Tag("Refused"),
      Busy: () => Tag("Busy"),
      Unavailable: () => this.#closed ? closedCryptoExecutor() : this.#openInline(job),
      Failed: (data) => Tag("Failed", data),
    });
  }

  async digest(
    plaintext: Uint8Array,
    salt: Uint8Array,
  ): Promise<ResourceCryptoDigestOutcome> {
    if (this.#closed) {
      return closedCryptoExecutor();
    }
    const outcome = await this.#pool.donateDigest(plaintext, salt);
    return match(outcome, {
      Digested: (data) => Tag("Digested", data),
      Busy: () => Tag("Busy"),
      Unavailable: () => this.#closed ? closedCryptoExecutor() : this.#digestInline(plaintext, salt),
      Failed: (data) => Tag("Failed", data),
    });
  }

  close(): void {
    this.#closed = true;
    this.#pool.close();
  }

  async #sealInline(job: ResourceSealCryptoJob): Promise<ResourceCryptoSealOutcome> {
    try {
      const sealed = await this.#sealer.seal(job);
      return Tag("Sealed", { sealed, plaintext: job.plaintext });
    } catch (error) {
      return cryptoFailed(error);
    }
  }

  async #openInline(job: ResourceOpenCryptoJob): Promise<ResourceCryptoOpenOutcome> {
    try {
      return await this.#opener.open(job);
    } catch (error) {
      return cryptoFailed(error);
    }
  }

  async #digestInline(
    plaintext: Uint8Array,
    salt: Uint8Array,
  ): Promise<ResourceCryptoDigestOutcome> {
    try {
      const digests = await this.#digester.digest(plaintext, salt);
      return Tag("Digested", { plaintext: transferableBytes(plaintext), ...digests });
    } catch (error) {
      return cryptoFailed(error);
    }
  }
}

export class WebCryptoResourceWorkerPool {
  readonly #slots: WorkerSlot[] = [];
  readonly #queue: QueuedJob[] = [];
  #nextId = 1;
  #retainedJobs = 0;
  #retainedBytes = 0;
  #closed = false;
  #starting: number;
  #readyWorkers = 0;
  readonly #readiness: Promise<ResourceCryptoPoolReadiness>;
  #settleReadiness: ((readiness: ResourceCryptoPoolReadiness) => void) | undefined;

  constructor(workers: number) {
    if (!Number.isSafeInteger(workers) || workers < 1 || workers > MAXIMUM_CRYPTO_WORKERS) {
      throw new RangeError(`crypto workers must be between 1 and ${MAXIMUM_CRYPTO_WORKERS}`);
    }
    this.#starting = workers;
    this.#readiness = new Promise((settle) => {
      this.#settleReadiness = settle;
    });
    if (typeof Worker !== "function") {
      this.#starting = 0;
      const settleReadiness = this.#settleReadiness;
      if (settleReadiness !== undefined) {
        settleReadiness(Tag("Unavailable"));
      }
      this.#settleReadiness = undefined;
      return;
    }
    for (let index = 0; index < workers; index += 1) {
      this.#startWorker(index);
    }
  }

  ready(): Promise<ResourceCryptoPoolReadiness> {
    return this.#readiness;
  }

  donateSeal(job: ResourceSealCryptoJob): Promise<ResourceCryptoSealSettlement> {
    const plaintext = transferableBytes(job.plaintext);
    const signingKey = transferableBytes(job.signingKey);
    const encryptionKey = transferableBytes(job.encryptionKey);
    const id = this.#takeId();
    const request = Tag("Seal", {
      id,
      job: {
        linkId: job.linkId,
        plaintext,
        signingKey,
        encryptionKey,
        sealIv: job.sealIv,
      },
    });
    return this.#submit(Tag("Seal", {
      id,
      request,
      transfer: uniqueTransfers([plaintext, signingKey, encryptionKey]),
      bytes: plaintext.byteLength + signingKey.byteLength + encryptionKey.byteLength,
      settle: () => undefined,
    }));
  }

  donateOpen(job: ResourceOpenCryptoJob): Promise<ResourceCryptoOpenSettlement> {
    const sealed = transferableBytes(job.sealed);
    const signingKey = transferableBytes(job.signingKey);
    const encryptionKey = transferableBytes(job.encryptionKey);
    const id = this.#takeId();
    const request = Tag("Open", {
      id,
      job: {
        linkId: job.linkId,
        sealed,
        signingKey,
        encryptionKey,
      },
    });
    return this.#submit(Tag("Open", {
      id,
      request,
      transfer: uniqueTransfers([sealed, signingKey, encryptionKey]),
      bytes: sealed.byteLength + signingKey.byteLength + encryptionKey.byteLength,
      settle: () => undefined,
    }));
  }

  donateDigest(
    plaintextBytes: Uint8Array,
    saltBytes: Uint8Array,
  ): Promise<ResourceCryptoDigestSettlement> {
    const plaintext = transferableBytes(plaintextBytes);
    const salt = transferableBytes(saltBytes);
    const id = this.#takeId();
    const request = Tag("Digest", { id, plaintext, salt });
    return this.#submit(Tag("Digest", {
      id,
      request,
      transfer: uniqueTransfers([plaintext]),
      bytes: plaintext.byteLength + saltBytes.byteLength,
      settle: () => undefined,
    }));
  }

  close(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    for (const slot of this.#slots) {
      globalThis.clearTimeout(slot.startupTimeout);
      match(slot.state, {
        Starting: () => undefined,
        Ready: () => undefined,
        Running: (job) => this.#settle(
          job,
          Tag("Failed", { detail: "crypto Worker terminated during an operation" }),
        ),
        Failed: () => undefined,
      });
      slot.state = Tag("Failed");
      slot.worker.terminate();
    }
    for (const job of this.#queue.splice(0)) {
      this.#settle(job, Tag("Unavailable"));
    }
    this.#starting = 0;
    this.#readyWorkers = 0;
    this.#finishReadiness();
  }

  #submit(job: Extract<QueuedJob, { readonly tag: "Seal" }>): Promise<ResourceCryptoSealSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "Open" }>): Promise<ResourceCryptoOpenSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "Digest" }>): Promise<ResourceCryptoDigestSettlement>;
  #submit(job: QueuedJob): Promise<ResourceCryptoSettlement> {
    if (this.#closed || this.#availableWorkers() === 0) {
      return Promise.resolve(Tag("Unavailable"));
    }
    const data = job.data;
    if (
      this.#retainedJobs >= MAXIMUM_PENDING_CRYPTO_JOBS ||
      this.#retainedBytes + data.bytes > MAXIMUM_PENDING_CRYPTO_BYTES
    ) {
      return Promise.resolve(Tag("Busy"));
    }
    return new Promise((settle) => {
      const admitted = match(job, {
        Seal: (queued) => Tag("Seal", { ...queued, settle }),
        Open: (queued) => Tag("Open", { ...queued, settle }),
        Digest: (queued) => Tag("Digest", { ...queued, settle }),
      });
      this.#retainedJobs += 1;
      this.#retainedBytes += data.bytes;
      this.#queue.push(admitted);
      this.#dispatch();
    });
  }

  #startWorker(index: number): void {
    let worker: Worker;
    try {
      worker = new Worker(new URL("./resource_crypto_worker.js", import.meta.url), {
        type: "module",
        name: `prns-crypto-${index + 1}`,
      });
    } catch {
      this.#starting -= 1;
      this.#finishReadiness();
      return;
    }
    let slot: WorkerSlot;
    const startupTimeout = globalThis.setTimeout(() => {
      this.#fail(slot);
    }, RESOURCE_CRYPTO_WORKER_START_TIMEOUT_MILLIS);
    slot = { worker, startupTimeout, state: Tag("Starting") };
    this.#slots.push(slot);
    worker.addEventListener("message", (event: MessageEvent<unknown>) => {
      this.#receive(slot, event.data);
    });
    worker.addEventListener("error", () => {
      this.#fail(slot);
    });
    worker.addEventListener("messageerror", () => {
      this.#fail(slot);
    });
  }

  #receive(slot: WorkerSlot, raw: unknown): void {
    if (!isWorkerResponse(raw)) {
      this.#fail(slot);
      return;
    }
    match(raw, {
      Ready: () => {
        if (slot.state.tag !== "Starting") {
          this.#fail(slot);
          return;
        }
        globalThis.clearTimeout(slot.startupTimeout);
        slot.state = Tag("Ready");
        this.#starting -= 1;
        this.#readyWorkers += 1;
        this.#finishReadiness();
        this.#dispatch();
      },
      Sealed: (response) => {
        this.#complete(slot, response.id, "Seal", Tag("Sealed", {
          sealed: new Uint8Array(response.sealed),
          plaintext: new Uint8Array(response.plaintext),
        }));
      },
      Opened: (response) => {
        this.#complete(
          slot,
          response.id,
          "Open",
          Tag("Opened", new Uint8Array(response.plaintext)),
        );
      },
      Refused: ({ id }) => {
        this.#complete(slot, id, "Open", Tag("Refused"));
      },
      Digested: (response) => {
        this.#complete(slot, response.id, "Digest", Tag("Digested", {
          plaintext: new Uint8Array(response.plaintext),
          hash: new Uint8Array(response.hash),
          proof: new Uint8Array(response.proof),
        }));
      },
      Failed: ({ id, detail }) => {
        this.#complete(slot, id, undefined, Tag("Failed", { detail }));
      },
    });
  }

  #complete(
    slot: WorkerSlot,
    id: number,
    expected: QueuedJob["tag"] | undefined,
    settlement: ResourceCryptoSettlement,
  ): void {
    if (
      slot.state.tag !== "Running" ||
      slot.state.data.data.id !== id ||
      (expected !== undefined && slot.state.data.tag !== expected)
    ) {
      this.#fail(slot);
      return;
    }
    const job = slot.state.data;
    slot.state = Tag("Ready");
    this.#settle(job, settlement);
    this.#dispatch();
  }

  #dispatch(): void {
    if (this.#closed) {
      return;
    }
    for (const slot of this.#slots) {
      if (slot.state.tag !== "Ready") {
        continue;
      }
      const job = this.#queue.shift();
      if (job === undefined) {
        return;
      }
      slot.state = Tag("Running", job);
      const { request, transfer } = job.data;
      try {
        slot.worker.postMessage(request, transfer);
      } catch {
        this.#fail(slot);
      }
    }
  }

  #fail(slot: WorkerSlot): void {
    if (slot.state.tag === "Failed") {
      return;
    }
    globalThis.clearTimeout(slot.startupTimeout);
    match(slot.state, {
      Starting: () => {
        this.#starting -= 1;
      },
      Ready: () => {
        this.#readyWorkers -= 1;
      },
      Running: (job) => {
        this.#readyWorkers -= 1;
        this.#settle(
          job,
          Tag("Failed", { detail: "crypto Worker became unavailable during an operation" }),
        );
      },
      Failed: () => undefined,
    });
    slot.state = Tag("Failed");
    slot.worker.terminate();
    this.#finishReadiness();
    if (this.#availableWorkers() === 0) {
      for (const job of this.#queue.splice(0)) {
        this.#settle(job, Tag("Unavailable"));
      }
      return;
    }
    this.#dispatch();
  }

  #settle(job: QueuedJob, settlement: ResourceCryptoSettlement): void {
    this.#retainedJobs -= 1;
    this.#retainedBytes -= job.data.bytes;
    job.data.settle(settlement);
  }

  #availableWorkers(): number {
    return this.#slots.reduce(
      (count, slot) => slot.state.tag === "Failed" ? count : count + 1,
      0,
    );
  }

  #finishReadiness(): void {
    if (this.#starting !== 0 || this.#settleReadiness === undefined) {
      return;
    }
    this.#settleReadiness(
      this.#readyWorkers === 0
        ? Tag("Unavailable")
        : Tag("Ready", { workers: this.#readyWorkers }),
    );
    this.#settleReadiness = undefined;
  }

  #takeId(): number {
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    return id;
  }
}

function transferableBytes(bytes: Uint8Array): OwnedBytes {
  if (
    bytes.buffer instanceof ArrayBuffer &&
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength
  ) {
    return bytes as OwnedBytes;
  }
  return bytes.slice() as OwnedBytes;
}

function uniqueTransfers(views: readonly OwnedBytes[]): Transferable[] {
  return [...new Set(views.map((view) => view.buffer))];
}

function cryptoFailed(error: unknown): Extract<ResourceCryptoPoolFailure, { readonly tag: "Failed" }> {
  return Tag("Failed", {
    detail: error instanceof Error ? error.message : String(error),
  });
}

function closedCryptoExecutor(): Extract<ResourceCryptoPoolFailure, { readonly tag: "Failed" }> {
  return Tag("Failed", { detail: "resource crypto executor is closed" });
}

function isWorkerResponse(raw: unknown): raw is ResourceCryptoWorkerResponse {
  if (typeof raw !== "object" || raw === null || !("tag" in raw)) {
    return false;
  }
  const response = raw as Record<string, unknown>;
  if (
    response.tag !== "Ready" &&
    response.tag !== "Sealed" &&
    response.tag !== "Opened" &&
    response.tag !== "Refused" &&
    response.tag !== "Digested" &&
    response.tag !== "Failed"
  ) {
    return false;
  }
  if (response.tag === "Ready") {
    return true;
  }
  if (typeof response.data !== "object" || response.data === null) {
    return false;
  }
  const data = response.data as Record<string, unknown>;
  if (!Number.isSafeInteger(data.id)) {
    return false;
  }
  return match_into<boolean>().from(response as unknown as ResourceCryptoWorkerResponse, {
    Ready: () => true,
    Sealed: ({ sealed, plaintext }) =>
      sealed instanceof ArrayBuffer && plaintext instanceof ArrayBuffer,
    Opened: ({ plaintext }) => plaintext instanceof ArrayBuffer,
    Refused: () => true,
    Digested: ({ plaintext, hash, proof }) =>
      plaintext instanceof ArrayBuffer &&
      hash instanceof ArrayBuffer &&
      hash.byteLength === 32 &&
      proof instanceof ArrayBuffer &&
      proof.byteLength === 32,
    Failed: ({ detail }) => typeof detail === "string",
  });
}

import { Tag, match } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import type {
  CryptoPoolFailure,
  Ed25519VerifySettlement,
  LinkProofVerifySettlement,
} from "./crypto_pool.js";
import type {
  PortableCryptoWorkerJob,
  PortableCryptoWorkerOutcome,
  PortableCryptoWorkerRequest,
  PortableCryptoWorkerResponse,
} from "./portable_crypto_worker_protocol.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

type QueuedJob =
  | Tagged<
      "AnnounceVerify",
      {
        readonly id: number;
        readonly job: Extract<
          PortableCryptoWorkerJob,
          { readonly tag: "AnnounceVerify" }
        >;
        readonly bytes: number;
        readonly settle: (settlement: PortableCryptoSettlement) => void;
      }
    >
  | Tagged<
      "LinkProofVerify",
      {
        readonly id: number;
        readonly job: Extract<
          PortableCryptoWorkerJob,
          { readonly tag: "LinkProofVerify" }
        >;
        readonly bytes: number;
        readonly settle: (settlement: PortableCryptoSettlement) => void;
      }
    >;

type PortableCryptoSettlement =
  | Ed25519VerifySettlement
  | LinkProofVerifySettlement;

type WorkerState =
  | Tagged<"Starting">
  | Tagged<"Ready">
  | Tagged<"Running", { readonly jobs: readonly QueuedJob[] }>
  | Tagged<"Failed">;

type WorkerSlot = {
  readonly worker: Worker;
  readonly startupTimeout: number;
  state: WorkerState;
};

export type PortableCryptoPoolReadiness =
  | Tagged<"Ready", { readonly workers: number }>
  | Tagged<"Unavailable">;

const MAXIMUM_PORTABLE_CRYPTO_WORKERS = 16;
const MAXIMUM_PENDING_PORTABLE_CRYPTO_JOBS = 64;
const MAXIMUM_PENDING_PORTABLE_CRYPTO_BYTES = 1024 * 1024;
const MAXIMUM_PORTABLE_CRYPTO_BATCH_JOBS = 16;
const PORTABLE_CRYPTO_WORKER_START_TIMEOUT_MILLIS = 10_000;

export class PortableCryptoWorkerPool {
  readonly #slots: WorkerSlot[] = [];
  readonly #queue: QueuedJob[] = [];
  readonly #wasmModuleUrl: string | undefined;
  readonly #readiness: Promise<PortableCryptoPoolReadiness>;
  #settleReadiness:
    | ((readiness: PortableCryptoPoolReadiness) => void)
    | undefined;
  #nextId = 1;
  #retainedJobs = 0;
  #retainedBytes = 0;
  #starting: number;
  #readyWorkers = 0;
  #dispatchScheduled = false;
  #closed = false;

  constructor(workers: number, wasmModuleUrl?: string) {
    if (
      !Number.isSafeInteger(workers) ||
      workers < 1 ||
      workers > MAXIMUM_PORTABLE_CRYPTO_WORKERS
    ) {
      throw new RangeError(
        `portable crypto workers must be between 1 and ${MAXIMUM_PORTABLE_CRYPTO_WORKERS}`,
      );
    }
    this.#wasmModuleUrl = wasmModuleUrl;
    this.#starting = workers;
    this.#readiness = new Promise((settle) => {
      this.#settleReadiness = settle;
    });
    if (typeof Worker !== "function") {
      this.#starting = 0;
      this.#finishReadiness();
      return;
    }
    for (let index = 0; index < workers; index += 1) {
      this.#startWorker(index);
    }
  }

  ready(): Promise<PortableCryptoPoolReadiness> {
    return this.#readiness;
  }

  verifyEd25519(
    publicKeyBytes: Uint8Array,
    messageBytes: Uint8Array,
    signatureBytes: Uint8Array,
  ): Promise<Ed25519VerifySettlement> {
    const publicKey = ownedBytes(publicKeyBytes);
    const message = ownedBytes(messageBytes);
    const signature = ownedBytes(signatureBytes);
    const id = this.#takeId();
    const job = Tag("AnnounceVerify", { id, publicKey, message, signature });
    return this.#submit(Tag("AnnounceVerify", {
      id,
      job,
      bytes: publicKey.byteLength + message.byteLength + signature.byteLength,
      settle: () => undefined,
    }));
  }

  verifyLinkProof(
    publicKeyBytes: Uint8Array,
    messageBytes: Uint8Array,
    signatureBytes: Uint8Array,
    secretScalarBytes: Uint8Array,
    peerPublicKeyBytes: Uint8Array,
  ): Promise<LinkProofVerifySettlement> {
    const publicKey = ownedBytes(publicKeyBytes);
    const message = ownedBytes(messageBytes);
    const signature = ownedBytes(signatureBytes);
    const secretScalar = ownedBytes(secretScalarBytes);
    const peerPublicKey = ownedBytes(peerPublicKeyBytes);
    const id = this.#takeId();
    const job = Tag("LinkProofVerify", {
      id,
      publicKey,
      message,
      signature,
      secretScalar,
      peerPublicKey,
    });
    return this.#submit(Tag("LinkProofVerify", {
      id,
      job,
      bytes:
        publicKey.byteLength +
        message.byteLength +
        signature.byteLength +
        secretScalar.byteLength +
        peerPublicKey.byteLength,
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
        Running: ({ jobs }) => {
          for (const job of jobs) {
            eraseSecret(job);
            this.#settle(job, Tag("Failed", {
              detail: "portable crypto Worker terminated during an operation",
            }));
          }
        },
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

  #submit(
    job: Extract<QueuedJob, { readonly tag: "AnnounceVerify" }>,
  ): Promise<Ed25519VerifySettlement>;
  #submit(
    job: Extract<QueuedJob, { readonly tag: "LinkProofVerify" }>,
  ): Promise<LinkProofVerifySettlement>;
  #submit(job: QueuedJob): Promise<PortableCryptoSettlement> {
    if (this.#closed || this.#availableWorkers() === 0) {
      return Promise.resolve(Tag("Unavailable"));
    }
    if (
      this.#retainedJobs >= MAXIMUM_PENDING_PORTABLE_CRYPTO_JOBS ||
      this.#retainedBytes + job.data.bytes > MAXIMUM_PENDING_PORTABLE_CRYPTO_BYTES
    ) {
      return Promise.resolve(Tag("Busy"));
    }
    return new Promise((settle) => {
      const admitted = match(job, {
        AnnounceVerify: (queued) => Tag("AnnounceVerify", { ...queued, settle }),
        LinkProofVerify: (queued) => Tag("LinkProofVerify", { ...queued, settle }),
      });
      this.#retainedJobs += 1;
      this.#retainedBytes += job.data.bytes;
      this.#queue.push(admitted);
      this.#scheduleDispatch();
    });
  }

  #startWorker(index: number): void {
    let worker: Worker;
    try {
      worker = new Worker(new URL("./portable_crypto_worker.js", import.meta.url), {
        type: "module",
        name: `prns-portable-crypto-${index + 1}`,
      });
    } catch {
      this.#starting -= 1;
      this.#finishReadiness();
      return;
    }
    let slot: WorkerSlot;
    const startupTimeout = globalThis.setTimeout(() => {
      this.#fail(slot);
    }, PORTABLE_CRYPTO_WORKER_START_TIMEOUT_MILLIS);
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
    const request: PortableCryptoWorkerRequest = Tag("Initialize", {
      ...(this.#wasmModuleUrl === undefined
        ? {}
        : { wasmModuleUrl: this.#wasmModuleUrl }),
    });
    try {
      worker.postMessage(request);
    } catch {
      this.#fail(slot);
    }
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
        this.#scheduleDispatch();
      },
      Settled: ({ outcomes }) => {
        if (slot.state.tag !== "Running") {
          this.#fail(slot);
          return;
        }
        const jobs = slot.state.data.jobs;
        if (!settlementsMatch(jobs, outcomes)) {
          this.#fail(slot);
          return;
        }
        slot.state = Tag("Ready");
        for (let index = 0; index < jobs.length; index += 1) {
          this.#settleOutcome(jobs[index]!, outcomes[index]!);
        }
        this.#scheduleDispatch();
      },
      InitializationFailed: () => {
        this.#fail(slot);
      },
    });
  }

  #scheduleDispatch(): void {
    if (this.#closed || this.#dispatchScheduled) {
      return;
    }
    this.#dispatchScheduled = true;
    globalThis.queueMicrotask(() => {
      this.#dispatchScheduled = false;
      this.#dispatch();
    });
  }

  #dispatch(): void {
    if (this.#closed || this.#queue.length === 0) {
      return;
    }
    const readySlots = this.#slots.filter((slot) => slot.state.tag === "Ready");
    for (let index = 0; index < readySlots.length && this.#queue.length > 0; index += 1) {
      const slot = readySlots[index]!;
      const remainingSlots = readySlots.length - index;
      const batchLength = Math.min(
        MAXIMUM_PORTABLE_CRYPTO_BATCH_JOBS,
        Math.ceil(this.#queue.length / remainingSlots),
      );
      const jobs = this.#queue.splice(0, batchLength);
      slot.state = Tag("Running", { jobs });
      const request: PortableCryptoWorkerRequest = Tag("Perform", {
        jobs: jobs.map((job) => job.data.job),
      });
      try {
        slot.worker.postMessage(request);
        for (const job of jobs) {
          eraseSecret(job);
        }
      } catch {
        this.#fail(slot);
      }
    }
  }

  #settleOutcome(job: QueuedJob, outcome: PortableCryptoWorkerOutcome): void {
    const settlement = match(outcome, {
      AnnounceValid: () => Tag("Valid"),
      AnnounceInvalid: () => Tag("Invalid"),
      LinkProofVerified: ({ sharedSecret }) =>
        Tag("Verified", { sharedSecret }),
      LinkProofInvalid: () => Tag("Invalid"),
      OperationFailed: ({ detail }) => Tag("Failed", { detail }),
    });
    this.#settle(job, settlement);
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
      Running: ({ jobs }) => {
        this.#readyWorkers -= 1;
        for (const job of jobs) {
          eraseSecret(job);
          this.#settle(job, Tag("Failed", {
            detail: "portable crypto Worker became unavailable during an operation",
          }));
        }
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
    } else {
      this.#scheduleDispatch();
    }
  }

  #settle(job: QueuedJob, settlement: PortableCryptoSettlement): void {
    this.#retainedJobs -= 1;
    this.#retainedBytes -= job.data.bytes;
    job.data.settle(settlement);
  }

  #availableWorkers(): number {
    return this.#slots.reduce(
      (count, slot) => count + (slot.state.tag === "Failed" ? 0 : 1),
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

function settlementsMatch(
  jobs: readonly QueuedJob[],
  outcomes: readonly PortableCryptoWorkerOutcome[],
): boolean {
  if (jobs.length !== outcomes.length) {
    return false;
  }
  return jobs.every((job, index) => {
    const outcome = outcomes[index]!;
    if (job.data.id !== outcome.data.id) {
      return false;
    }
    if (outcome.tag === "OperationFailed") {
      return true;
    }
    return job.tag === "AnnounceVerify"
      ? outcome.tag === "AnnounceValid" || outcome.tag === "AnnounceInvalid"
      : outcome.tag === "LinkProofVerified" || outcome.tag === "LinkProofInvalid";
  });
}

function eraseSecret(job: QueuedJob): void {
  if (job.tag === "LinkProofVerify") {
    job.data.job.data.secretScalar.fill(0);
  }
}

function ownedBytes(bytes: Uint8Array): OwnedBytes {
  if (
    bytes.buffer instanceof ArrayBuffer &&
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength
  ) {
    return bytes as OwnedBytes;
  }
  return bytes.slice() as OwnedBytes;
}

function isWorkerResponse(raw: unknown): raw is PortableCryptoWorkerResponse {
  if (typeof raw !== "object" || raw === null || !("tag" in raw)) {
    return false;
  }
  const response = raw as Record<string, unknown>;
  if (response.tag === "Ready") {
    return response.data === undefined;
  }
  if (typeof response.data !== "object" || response.data === null) {
    return false;
  }
  const data = response.data as Record<string, unknown>;
  if (response.tag === "InitializationFailed") {
    return typeof data.detail === "string";
  }
  if (response.tag !== "Settled" || !Array.isArray(data.outcomes)) {
    return false;
  }
  return data.outcomes.every(isWorkerOutcome);
}

function isWorkerOutcome(raw: unknown): raw is PortableCryptoWorkerOutcome {
  if (typeof raw !== "object" || raw === null || !("tag" in raw)) {
    return false;
  }
  const outcome = raw as Record<string, unknown>;
  if (typeof outcome.data !== "object" || outcome.data === null) {
    return false;
  }
  const data = outcome.data as Record<string, unknown>;
  if (!Number.isSafeInteger(data.id) || (data.id as number) < 1) {
    return false;
  }
  if (
    outcome.tag === "AnnounceValid" ||
    outcome.tag === "AnnounceInvalid" ||
    outcome.tag === "LinkProofInvalid"
  ) {
    return true;
  }
  if (outcome.tag === "OperationFailed") {
    return typeof data.detail === "string";
  }
  return outcome.tag === "LinkProofVerified" &&
    data.sharedSecret instanceof Uint8Array &&
    data.sharedSecret.buffer instanceof ArrayBuffer &&
    data.sharedSecret.byteLength === 32;
}

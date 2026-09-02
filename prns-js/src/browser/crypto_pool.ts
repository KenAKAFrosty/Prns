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
  CryptoWorkerRequest,
  CryptoWorkerResponse,
} from "./crypto_worker_protocol.js";
import type {
  HkdfSha256Job,
  PrnsWebCryptoCompatibility,
  SignatureVerification,
  WebCryptoPrimitiveCompatibility,
} from "./protocol_crypto.js";
import { PortableCryptoWorkerPool } from "./portable_crypto_pool.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type CryptoPoolFailure =
  | Tagged<"Busy">
  | Tagged<"Unavailable">
  | Tagged<"Failed", { readonly detail: string }>;

export type ResourceCryptoPoolFailure = CryptoPoolFailure;

export type ResourceCryptoSealSettlement =
  | Tagged<
      "Sealed",
      {
        readonly sealed: OwnedBytes;
        readonly plaintext: OwnedBytes;
      }
    >
  | CryptoPoolFailure;

export type ResourceCryptoOpenSettlement =
  | Tagged<"Opened", OwnedBytes>
  | Tagged<"Refused">
  | CryptoPoolFailure;

export type ResourceCryptoSealAndDigestSettlement =
  | Tagged<
      "SealedAndDigested",
      {
        readonly sealed: OwnedBytes;
        readonly plaintext: OwnedBytes;
        readonly hash: OwnedBytes;
        readonly proof: OwnedBytes;
      }
    >
  | CryptoPoolFailure;

export type ResourceCryptoOpenAndDigestSettlement =
  | Tagged<
      "OpenedAndDigested",
      {
        readonly plaintext: OwnedBytes;
        readonly hash: OwnedBytes;
        readonly proof: OwnedBytes;
      }
    >
  | Tagged<"Refused">
  | CryptoPoolFailure;

export type ResourceCryptoDigestSettlement =
  | Tagged<
      "Digested",
      {
        readonly plaintext: OwnedBytes;
        readonly hash: OwnedBytes;
        readonly proof: OwnedBytes;
      }
    >
  | CryptoPoolFailure;

export type Ed25519SignSettlement =
  | Tagged<"Signed", { readonly signature: OwnedBytes }>
  | CryptoPoolFailure;

export type Ed25519VerifySettlement = SignatureVerification | CryptoPoolFailure;

export type X25519DeriveSettlement =
  | Tagged<"Derived", { readonly sharedSecret: OwnedBytes }>
  | CryptoPoolFailure;

export type LinkProofVerifySettlement =
  | Tagged<"Verified", { readonly sharedSecret: OwnedBytes }>
  | Tagged<"Invalid">
  | CryptoPoolFailure;

export type HkdfSha256DeriveSettlement =
  | Tagged<"Derived", { readonly keyMaterial: OwnedBytes }>
  | CryptoPoolFailure;

export type CryptoPoolReadiness =
  | Tagged<
      "Ready",
      {
        readonly workers: number;
        readonly compatibility: PrnsWebCryptoCompatibility;
      }
    >
  | Tagged<"Unavailable">;

export type ResourceCryptoPoolReadiness = CryptoPoolReadiness;

export type ResourceCryptoSealOutcome = Extract<
  ResourceCryptoSealSettlement,
  { readonly tag: "Sealed" | "Busy" | "Failed" }
>;

export type ResourceCryptoOpenOutcome = Extract<
  ResourceCryptoOpenSettlement,
  { readonly tag: "Opened" | "Refused" | "Busy" | "Failed" }
>;

export type ResourceCryptoSealAndDigestOutcome = Extract<
  ResourceCryptoSealAndDigestSettlement,
  { readonly tag: "SealedAndDigested" | "Busy" | "Failed" }
>;

export type ResourceCryptoOpenAndDigestOutcome = Extract<
  ResourceCryptoOpenAndDigestSettlement,
  { readonly tag: "OpenedAndDigested" | "Refused" | "Busy" | "Failed" }
>;

export type ResourceCryptoDigestOutcome = Extract<
  ResourceCryptoDigestSettlement,
  { readonly tag: "Digested" | "Busy" | "Failed" }
>;

type CryptoSettlement =
  | ResourceCryptoSealSettlement
  | ResourceCryptoOpenSettlement
  | ResourceCryptoSealAndDigestSettlement
  | ResourceCryptoOpenAndDigestSettlement
  | ResourceCryptoDigestSettlement
  | Ed25519SignSettlement
  | Ed25519VerifySettlement
  | X25519DeriveSettlement
  | LinkProofVerifySettlement
  | HkdfSha256DeriveSettlement;

type QueuedJob =
  | Tagged<
      "Seal",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "Seal" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "Open",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "Open" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "SealAndDigest",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "SealAndDigest" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "OpenAndDigest",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "OpenAndDigest" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "Digest",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "Digest" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "Ed25519Sign",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "Ed25519Sign" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "Ed25519Verify",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "Ed25519Verify" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "X25519Derive",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "X25519Derive" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "LinkProofVerify",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "LinkProofVerify" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
      }
    >
  | Tagged<
      "HkdfSha256Derive",
      {
        readonly id: number;
        readonly request: Extract<CryptoWorkerRequest, { readonly tag: "HkdfSha256Derive" }>;
        readonly transfer: Transferable[];
        readonly bytes: number;
        readonly settle: (settlement: CryptoSettlement) => void;
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
  compatibility: PrnsWebCryptoCompatibility | undefined;
  state: WorkerState;
};

const MAXIMUM_CRYPTO_WORKERS = 16;
const MAXIMUM_PENDING_CRYPTO_JOBS = 16;
const MAXIMUM_PENDING_CRYPTO_BYTES = 64 * 1_024 * 1_024;
const CRYPTO_WORKER_START_TIMEOUT_MILLIS = 5_000;

export class BrowserCryptoExecutor {
  readonly #pool: WebCryptoWorkerPool;
  readonly #portablePool: PortableCryptoWorkerPool | undefined;
  readonly #sealer = new WebCryptoResourceSealer();
  readonly #opener = new WebCryptoResourceOpener();
  readonly #digester = new WebCryptoResourceDigester();
  #closed = false;

  constructor(
    webCryptoWorkers: number,
    portableWasm?: {
      readonly workers: number;
      readonly moduleUrl?: string;
    },
  ) {
    const pool = new WebCryptoWorkerPool(webCryptoWorkers);
    let portablePool: PortableCryptoWorkerPool | undefined;
    try {
      portablePool = portableWasm === undefined
        ? undefined
        : new PortableCryptoWorkerPool(portableWasm.workers, portableWasm.moduleUrl);
    } catch (error) {
      pool.close();
      throw error;
    }
    this.#pool = pool;
    this.#portablePool = portablePool;
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

  async sealAndDigest(
    job: ResourceSealCryptoJob,
    salt: Uint8Array,
  ): Promise<ResourceCryptoSealAndDigestOutcome> {
    if (this.#closed) {
      return closedCryptoExecutor();
    }
    const outcome = await this.#pool.donateSealAndDigest(job, salt);
    return match(outcome, {
      SealedAndDigested: (data) => Tag("SealedAndDigested", data),
      Busy: () => Tag("Busy"),
      Unavailable: () => this.#closed
        ? closedCryptoExecutor()
        : this.#sealAndDigestInline(job, salt),
      Failed: (data) => Tag("Failed", data),
    });
  }

  async openAndDigest(
    job: ResourceOpenCryptoJob,
    salt: Uint8Array,
  ): Promise<ResourceCryptoOpenAndDigestOutcome> {
    if (this.#closed) {
      return closedCryptoExecutor();
    }
    const outcome = await this.#pool.donateOpenAndDigest(job, salt);
    return match(outcome, {
      OpenedAndDigested: (data) => Tag("OpenedAndDigested", data),
      Refused: () => Tag("Refused"),
      Busy: () => Tag("Busy"),
      Unavailable: () => this.#closed
        ? closedCryptoExecutor()
        : this.#openAndDigestInline(job, salt),
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

  async verifyEd25519(
    publicKey: Uint8Array,
    message: Uint8Array,
    signature: Uint8Array,
  ): Promise<Ed25519VerifySettlement> {
    if (this.#closed) {
      return Promise.resolve(closedCryptoExecutor());
    }
    const portablePool = this.#portablePool;
    if (portablePool === undefined) {
      return this.#pool.donateEd25519Verify(publicKey, message, signature);
    }
    const outcome = await portablePool.verifyEd25519(publicKey, message, signature);
    return match(outcome, {
      Valid: () => Tag("Valid"),
      Invalid: () => Tag("Invalid"),
      Busy: () => this.#pool.donateEd25519Verify(publicKey, message, signature),
      Unavailable: () => this.#pool.donateEd25519Verify(publicKey, message, signature),
      Failed: (data) => Tag("Failed", data),
    });
  }

  async verifyLinkProof(
    publicKey: Uint8Array,
    message: Uint8Array,
    signature: Uint8Array,
    secretScalar: Uint8Array,
    peerPublicKey: Uint8Array,
  ): Promise<LinkProofVerifySettlement> {
    if (this.#closed) {
      return Promise.resolve(closedCryptoExecutor());
    }
    const portablePool = this.#portablePool;
    if (portablePool === undefined) {
      return this.#pool.donateLinkProofVerify(
        publicKey,
        message,
        signature,
        secretScalar,
        peerPublicKey,
      );
    }
    const outcome = await portablePool.verifyLinkProof(
      publicKey,
      message,
      signature,
      secretScalar,
      peerPublicKey,
    );
    return match(outcome, {
      Verified: (data) => Tag("Verified", data),
      Invalid: () => Tag("Invalid"),
      Busy: () => this.#pool.donateLinkProofVerify(
        publicKey,
        message,
        signature,
        secretScalar,
        peerPublicKey,
      ),
      Unavailable: () => this.#pool.donateLinkProofVerify(
        publicKey,
        message,
        signature,
        secretScalar,
        peerPublicKey,
      ),
      Failed: (data) => Tag("Failed", data),
    });
  }

  close(): void {
    this.#closed = true;
    this.#pool.close();
    this.#portablePool?.close();
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

  async #sealAndDigestInline(
    job: ResourceSealCryptoJob,
    salt: Uint8Array,
  ): Promise<ResourceCryptoSealAndDigestOutcome> {
    try {
      const [sealed, digests] = await Promise.all([
        this.#sealer.seal(job),
        this.#digester.digest(job.plaintext, salt),
      ]);
      return Tag("SealedAndDigested", {
        sealed,
        plaintext: job.plaintext,
        ...digests,
      });
    } catch (error) {
      return cryptoFailed(error);
    }
  }

  async #openAndDigestInline(
    job: ResourceOpenCryptoJob,
    salt: Uint8Array,
  ): Promise<ResourceCryptoOpenAndDigestOutcome> {
    try {
      const opened = await this.#opener.open(job);
      if (opened.tag === "Refused") {
        return Tag("Refused");
      }
      const digests = await this.#digester.digest(opened.data, salt);
      return Tag("OpenedAndDigested", { plaintext: opened.data, ...digests });
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

export class WebCryptoWorkerPool {
  readonly #slots: WorkerSlot[] = [];
  readonly #queue: QueuedJob[] = [];
  #nextId = 1;
  #retainedJobs = 0;
  #retainedBytes = 0;
  #closed = false;
  #starting: number;
  #readyWorkers = 0;
  readonly #readiness: Promise<CryptoPoolReadiness>;
  #settleReadiness: ((readiness: CryptoPoolReadiness) => void) | undefined;

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

  ready(): Promise<CryptoPoolReadiness> {
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

  donateSealAndDigest(
    job: ResourceSealCryptoJob,
    saltBytes: Uint8Array,
  ): Promise<ResourceCryptoSealAndDigestSettlement> {
    const plaintext = transferableBytes(job.plaintext);
    const signingKey = transferableBytes(job.signingKey);
    const encryptionKey = transferableBytes(job.encryptionKey);
    const salt = transferableBytes(saltBytes);
    const id = this.#takeId();
    const request = Tag("SealAndDigest", {
      id,
      job: {
        linkId: job.linkId,
        plaintext,
        signingKey,
        encryptionKey,
        sealIv: job.sealIv,
      },
      salt,
    });
    return this.#submit(Tag("SealAndDigest", {
      id,
      request,
      transfer: uniqueTransfers([plaintext, signingKey, encryptionKey]),
      bytes:
        plaintext.byteLength +
        signingKey.byteLength +
        encryptionKey.byteLength +
        salt.byteLength,
      settle: () => undefined,
    }));
  }

  donateOpenAndDigest(
    job: ResourceOpenCryptoJob,
    saltBytes: Uint8Array,
  ): Promise<ResourceCryptoOpenAndDigestSettlement> {
    const sealed = transferableBytes(job.sealed);
    const signingKey = transferableBytes(job.signingKey);
    const encryptionKey = transferableBytes(job.encryptionKey);
    const salt = transferableBytes(saltBytes);
    const id = this.#takeId();
    const request = Tag("OpenAndDigest", {
      id,
      job: {
        linkId: job.linkId,
        sealed,
        signingKey,
        encryptionKey,
      },
      salt,
    });
    return this.#submit(Tag("OpenAndDigest", {
      id,
      request,
      transfer: uniqueTransfers([sealed, signingKey, encryptionKey]),
      bytes:
        sealed.byteLength +
        signingKey.byteLength +
        encryptionKey.byteLength +
        salt.byteLength,
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

  donateEd25519Sign(
    secretSeedBytes: Uint8Array,
    messageBytes: Uint8Array,
  ): Promise<Ed25519SignSettlement> {
    const secretSeed = transferableBytes(secretSeedBytes);
    const message = transferableBytes(messageBytes);
    const id = this.#takeId();
    const request = Tag("Ed25519Sign", { id, secretSeed, message });
    return this.#submit(Tag("Ed25519Sign", {
      id,
      request,
      transfer: uniqueTransfers([secretSeed, message]),
      bytes: secretSeed.byteLength + message.byteLength,
      settle: () => undefined,
    }));
  }

  donateEd25519Verify(
    publicKeyBytes: Uint8Array,
    messageBytes: Uint8Array,
    signatureBytes: Uint8Array,
  ): Promise<Ed25519VerifySettlement> {
    const publicKey = transferableBytes(publicKeyBytes);
    const message = transferableBytes(messageBytes);
    const signature = transferableBytes(signatureBytes);
    const id = this.#takeId();
    const request = Tag("Ed25519Verify", { id, publicKey, message, signature });
    return this.#submit(Tag("Ed25519Verify", {
      id,
      request,
      transfer: uniqueTransfers([publicKey, message, signature]),
      bytes: publicKey.byteLength + message.byteLength + signature.byteLength,
      settle: () => undefined,
    }));
  }

  donateX25519Derive(
    secretScalarBytes: Uint8Array,
    peerPublicKeyBytes: Uint8Array,
  ): Promise<X25519DeriveSettlement> {
    const secretScalar = transferableBytes(secretScalarBytes);
    const peerPublicKey = transferableBytes(peerPublicKeyBytes);
    const id = this.#takeId();
    const request = Tag("X25519Derive", { id, secretScalar, peerPublicKey });
    return this.#submit(Tag("X25519Derive", {
      id,
      request,
      transfer: uniqueTransfers([secretScalar, peerPublicKey]),
      bytes: secretScalar.byteLength + peerPublicKey.byteLength,
      settle: () => undefined,
    }));
  }

  donateLinkProofVerify(
    publicKeyBytes: Uint8Array,
    messageBytes: Uint8Array,
    signatureBytes: Uint8Array,
    secretScalarBytes: Uint8Array,
    peerPublicKeyBytes: Uint8Array,
  ): Promise<LinkProofVerifySettlement> {
    const publicKey = transferableBytes(publicKeyBytes);
    const message = transferableBytes(messageBytes);
    const signature = transferableBytes(signatureBytes);
    const secretScalar = transferableBytes(secretScalarBytes);
    const peerPublicKey = transferableBytes(peerPublicKeyBytes);
    const id = this.#takeId();
    const request = Tag("LinkProofVerify", {
      id,
      publicKey,
      message,
      signature,
      secretScalar,
      peerPublicKey,
    });
    return this.#submit(Tag("LinkProofVerify", {
      id,
      request,
      transfer: uniqueTransfers([
        publicKey,
        message,
        signature,
        secretScalar,
        peerPublicKey,
      ]),
      bytes:
        publicKey.byteLength +
        message.byteLength +
        signature.byteLength +
        secretScalar.byteLength +
        peerPublicKey.byteLength,
      settle: () => undefined,
    }));
  }

  donateHkdfSha256(job: HkdfSha256Job): Promise<HkdfSha256DeriveSettlement> {
    const inputKeyMaterial = transferableBytes(job.inputKeyMaterial);
    const salt = transferableBytes(job.salt);
    const info = transferableBytes(job.info);
    const id = this.#takeId();
    const request = Tag("HkdfSha256Derive", {
      id,
      inputKeyMaterial,
      salt,
      info,
      outputBytes: job.outputBytes,
    });
    return this.#submit(Tag("HkdfSha256Derive", {
      id,
      request,
      transfer: uniqueTransfers([inputKeyMaterial, salt, info]),
      bytes: inputKeyMaterial.byteLength + salt.byteLength + info.byteLength,
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
  #submit(job: Extract<QueuedJob, { readonly tag: "SealAndDigest" }>): Promise<ResourceCryptoSealAndDigestSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "OpenAndDigest" }>): Promise<ResourceCryptoOpenAndDigestSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "Digest" }>): Promise<ResourceCryptoDigestSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "Ed25519Sign" }>): Promise<Ed25519SignSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "Ed25519Verify" }>): Promise<Ed25519VerifySettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "X25519Derive" }>): Promise<X25519DeriveSettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "LinkProofVerify" }>): Promise<LinkProofVerifySettlement>;
  #submit(job: Extract<QueuedJob, { readonly tag: "HkdfSha256Derive" }>): Promise<HkdfSha256DeriveSettlement>;
  #submit(job: QueuedJob): Promise<CryptoSettlement> {
    if (this.#closed || this.#availableWorkersFor(job.tag) === 0) {
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
        SealAndDigest: (queued) => Tag("SealAndDigest", { ...queued, settle }),
        OpenAndDigest: (queued) => Tag("OpenAndDigest", { ...queued, settle }),
        Digest: (queued) => Tag("Digest", { ...queued, settle }),
        Ed25519Sign: (queued) => Tag("Ed25519Sign", { ...queued, settle }),
        Ed25519Verify: (queued) => Tag("Ed25519Verify", { ...queued, settle }),
        X25519Derive: (queued) => Tag("X25519Derive", { ...queued, settle }),
        LinkProofVerify: (queued) => Tag("LinkProofVerify", { ...queued, settle }),
        HkdfSha256Derive: (queued) => Tag("HkdfSha256Derive", { ...queued, settle }),
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
      worker = new Worker(new URL("./crypto_worker.js", import.meta.url), {
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
    }, CRYPTO_WORKER_START_TIMEOUT_MILLIS);
    slot = {
      worker,
      startupTimeout,
      compatibility: undefined,
      state: Tag("Starting"),
    };
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
      Ready: ({ compatibility }) => {
        if (slot.state.tag !== "Starting") {
          this.#fail(slot);
          return;
        }
        globalThis.clearTimeout(slot.startupTimeout);
        slot.compatibility = compatibility;
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
      SealedAndDigested: (response) => {
        this.#complete(slot, response.id, "SealAndDigest", Tag("SealedAndDigested", {
          sealed: new Uint8Array(response.sealed),
          plaintext: new Uint8Array(response.plaintext),
          hash: new Uint8Array(response.hash),
          proof: new Uint8Array(response.proof),
        }));
      },
      OpenedAndDigested: (response) => {
        this.#complete(slot, response.id, "OpenAndDigest", Tag("OpenedAndDigested", {
          plaintext: new Uint8Array(response.plaintext),
          hash: new Uint8Array(response.hash),
          proof: new Uint8Array(response.proof),
        }));
      },
      OpenAndDigestRefused: ({ id }) => {
        this.#complete(slot, id, "OpenAndDigest", Tag("Refused"));
      },
      Digested: (response) => {
        this.#complete(slot, response.id, "Digest", Tag("Digested", {
          plaintext: new Uint8Array(response.plaintext),
          hash: new Uint8Array(response.hash),
          proof: new Uint8Array(response.proof),
        }));
      },
      Ed25519Signed: ({ id, signature }) => {
        this.#complete(
          slot,
          id,
          "Ed25519Sign",
          Tag("Signed", { signature: new Uint8Array(signature) }),
        );
      },
      Ed25519Valid: ({ id }) => {
        this.#complete(slot, id, "Ed25519Verify", Tag("Valid"));
      },
      Ed25519Invalid: ({ id }) => {
        this.#complete(slot, id, "Ed25519Verify", Tag("Invalid"));
      },
      X25519Derived: ({ id, sharedSecret }) => {
        this.#complete(
          slot,
          id,
          "X25519Derive",
          Tag("Derived", { sharedSecret: new Uint8Array(sharedSecret) }),
        );
      },
      LinkProofVerified: ({ id, sharedSecret }) => {
        this.#complete(
          slot,
          id,
          "LinkProofVerify",
          Tag("Verified", { sharedSecret: new Uint8Array(sharedSecret) }),
        );
      },
      LinkProofInvalid: ({ id }) => {
        this.#complete(slot, id, "LinkProofVerify", Tag("Invalid"));
      },
      HkdfSha256Derived: ({ id, keyMaterial }) => {
        this.#complete(
          slot,
          id,
          "HkdfSha256Derive",
          Tag("Derived", { keyMaterial: new Uint8Array(keyMaterial) }),
        );
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
    settlement: CryptoSettlement,
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
      const index = this.#queue.findIndex((job) => workerSupports(slot, job.tag));
      if (index < 0) {
        continue;
      }
      const job = this.#queue.splice(index, 1)[0]!;
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
    this.#rejectUnsupportedQueue();
    this.#dispatch();
  }

  #settle(job: QueuedJob, settlement: CryptoSettlement): void {
    this.#retainedJobs -= 1;
    this.#retainedBytes -= job.data.bytes;
    job.data.settle(settlement);
  }

  #availableWorkersFor(job: QueuedJob["tag"]): number {
    return this.#slots.reduce(
      (count, slot) => {
        if (slot.state.tag === "Failed") {
          return count;
        }
        if (slot.state.tag === "Starting" || workerSupports(slot, job)) {
          return count + 1;
        }
        return count;
      },
      0,
    );
  }

  #finishReadiness(): void {
    if (this.#starting !== 0) {
      return;
    }
    this.#rejectUnsupportedQueue();
    if (this.#settleReadiness === undefined) {
      return;
    }
    this.#settleReadiness(
      this.#readyWorkers === 0
        ? Tag("Unavailable")
        : Tag("Ready", {
          workers: this.#readyWorkers,
          compatibility: poolCompatibility(this.#slots),
        }),
    );
    this.#settleReadiness = undefined;
  }

  #rejectUnsupportedQueue(): void {
    if (this.#starting !== 0) {
      return;
    }
    for (let index = this.#queue.length - 1; index >= 0; index -= 1) {
      const job = this.#queue[index]!;
      if (this.#availableWorkersFor(job.tag) !== 0) {
        continue;
      }
      this.#queue.splice(index, 1);
      this.#settle(job, Tag("Unavailable"));
    }
  }

  #takeId(): number {
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    return id;
  }
}

function workerSupports(slot: WorkerSlot, job: QueuedJob["tag"]): boolean {
  if (
    job === "Seal" ||
    job === "Open" ||
    job === "SealAndDigest" ||
    job === "OpenAndDigest" ||
    job === "Digest"
  ) {
    return true;
  }
  const compatibility = slot.compatibility;
  if (compatibility === undefined) {
    return false;
  }
  if (job === "LinkProofVerify") {
    return compatibility.ed25519Verify.tag === "Compatible" &&
      compatibility.x25519.tag === "Compatible";
  }
  const capability = job === "Ed25519Sign"
    ? compatibility.ed25519Sign
    : job === "Ed25519Verify"
    ? compatibility.ed25519Verify
    : job === "X25519Derive"
    ? compatibility.x25519
    : compatibility.hkdfSha256;
  return capability.tag === "Compatible";
}

function poolCompatibility(slots: readonly WorkerSlot[]): PrnsWebCryptoCompatibility {
  return {
    ed25519Sign: aggregateCompatibility(slots, "ed25519Sign"),
    ed25519Verify: aggregateCompatibility(slots, "ed25519Verify"),
    x25519: aggregateCompatibility(slots, "x25519"),
    hkdfSha256: aggregateCompatibility(slots, "hkdfSha256"),
  };
}

function aggregateCompatibility(
  slots: readonly WorkerSlot[],
  operation: keyof PrnsWebCryptoCompatibility,
): WebCryptoPrimitiveCompatibility {
  let unavailable: WebCryptoPrimitiveCompatibility | undefined;
  for (const slot of slots) {
    const capability = slot.compatibility?.[operation];
    if (capability?.tag === "Compatible") {
      return capability;
    }
    if (capability !== undefined && unavailable === undefined) {
      unavailable = capability;
    }
  }
  return unavailable ?? Tag("Unavailable", { detail: "no compatible crypto Worker is ready" });
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

function cryptoFailed(error: unknown): Extract<CryptoPoolFailure, { readonly tag: "Failed" }> {
  return Tag("Failed", {
    detail: error instanceof Error ? error.message : String(error),
  });
}

function closedCryptoExecutor(): Extract<CryptoPoolFailure, { readonly tag: "Failed" }> {
  return Tag("Failed", { detail: "crypto executor is closed" });
}

function isWorkerResponse(raw: unknown): raw is CryptoWorkerResponse {
  if (typeof raw !== "object" || raw === null || !("tag" in raw)) {
    return false;
  }
  const response = raw as Record<string, unknown>;
  if (
    response.tag !== "Ready" &&
    response.tag !== "Sealed" &&
    response.tag !== "Opened" &&
    response.tag !== "Refused" &&
    response.tag !== "SealedAndDigested" &&
    response.tag !== "OpenedAndDigested" &&
    response.tag !== "OpenAndDigestRefused" &&
    response.tag !== "Digested" &&
    response.tag !== "Ed25519Signed" &&
    response.tag !== "Ed25519Valid" &&
    response.tag !== "Ed25519Invalid" &&
    response.tag !== "X25519Derived" &&
    response.tag !== "LinkProofVerified" &&
    response.tag !== "LinkProofInvalid" &&
    response.tag !== "HkdfSha256Derived" &&
    response.tag !== "Failed"
  ) {
    return false;
  }
  if (response.tag === "Ready") {
    if (typeof response.data !== "object" || response.data === null) {
      return false;
    }
    return isCompatibility((response.data as Record<string, unknown>).compatibility);
  }
  if (typeof response.data !== "object" || response.data === null) {
    return false;
  }
  const data = response.data as Record<string, unknown>;
  if (!Number.isSafeInteger(data.id)) {
    return false;
  }
  return match_into<boolean>().from(response as unknown as CryptoWorkerResponse, {
    Ready: ({ compatibility }) => isCompatibility(compatibility),
    Sealed: ({ sealed, plaintext }) =>
      sealed instanceof ArrayBuffer && plaintext instanceof ArrayBuffer,
    Opened: ({ plaintext }) => plaintext instanceof ArrayBuffer,
    Refused: () => true,
    SealedAndDigested: ({ sealed, plaintext, hash, proof }) =>
      sealed instanceof ArrayBuffer &&
      plaintext instanceof ArrayBuffer &&
      hash instanceof ArrayBuffer &&
      hash.byteLength === 32 &&
      proof instanceof ArrayBuffer &&
      proof.byteLength === 32,
    OpenedAndDigested: ({ plaintext, hash, proof }) =>
      plaintext instanceof ArrayBuffer &&
      hash instanceof ArrayBuffer &&
      hash.byteLength === 32 &&
      proof instanceof ArrayBuffer &&
      proof.byteLength === 32,
    OpenAndDigestRefused: () => true,
    Digested: ({ plaintext, hash, proof }) =>
      plaintext instanceof ArrayBuffer &&
      hash instanceof ArrayBuffer &&
      hash.byteLength === 32 &&
      proof instanceof ArrayBuffer &&
      proof.byteLength === 32,
    Ed25519Signed: ({ signature }) =>
      signature instanceof ArrayBuffer && signature.byteLength === 64,
    Ed25519Valid: () => true,
    Ed25519Invalid: () => true,
    X25519Derived: ({ sharedSecret }) =>
      sharedSecret instanceof ArrayBuffer && sharedSecret.byteLength === 32,
    LinkProofVerified: ({ sharedSecret }) =>
      sharedSecret instanceof ArrayBuffer && sharedSecret.byteLength === 32,
    LinkProofInvalid: () => true,
    HkdfSha256Derived: ({ keyMaterial }) => keyMaterial instanceof ArrayBuffer,
    Failed: ({ detail }) => typeof detail === "string",
  });
}

function isCompatibility(raw: unknown): raw is PrnsWebCryptoCompatibility {
  if (typeof raw !== "object" || raw === null) {
    return false;
  }
  const compatibility = raw as Record<string, unknown>;
  return isPrimitiveCompatibility(compatibility.ed25519Sign) &&
    isPrimitiveCompatibility(compatibility.ed25519Verify) &&
    isPrimitiveCompatibility(compatibility.x25519) &&
    isPrimitiveCompatibility(compatibility.hkdfSha256);
}

function isPrimitiveCompatibility(raw: unknown): raw is WebCryptoPrimitiveCompatibility {
  if (typeof raw !== "object" || raw === null || !("tag" in raw)) {
    return false;
  }
  const capability = raw as Record<string, unknown>;
  if (capability.tag === "Compatible") {
    return true;
  }
  if (capability.tag !== "Unavailable") {
    return false;
  }
  if (typeof capability.data !== "object" || capability.data === null) {
    return false;
  }
  return typeof (capability.data as Record<string, unknown>).detail === "string";
}

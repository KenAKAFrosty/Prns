import { Tag } from "../casework.js";
import type { CryptoExecution } from "./crypto_execution.js";

export type ResourceCryptoExecution = CryptoExecution;

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type ResourceSealJob = {
  readonly commandId: bigint;
  readonly linkId: OwnedBytes;
  readonly streamNonce: OwnedBytes;
  readonly noncePrefixedBytes: number;
  readonly totalSegments: number;
  readonly plaintext: OwnedBytes;
  readonly signingKey: OwnedBytes;
  readonly encryptionKey: OwnedBytes;
  readonly sealIv: OwnedBytes;
  readonly salts: OwnedBytes;
  readonly promotionEntropy: OwnedBytes;
};

export type ResourceSealCryptoJob = Pick<
  ResourceSealJob,
  "linkId" | "plaintext" | "signingKey" | "encryptionKey" | "sealIv"
>;

export type ResourceSealBegin =
  | Tag<"Inline", { readonly commandId: bigint }>
  | Tag<"Seal", ResourceSealJob>;

export type ResourceOpenJob = {
  readonly linkId: OwnedBytes;
  readonly hash: OwnedBytes;
  readonly signingKey: OwnedBytes;
  readonly encryptionKey: OwnedBytes;
  readonly sealed: OwnedBytes;
  readonly hashPlan: ResourceOpenHashPlan;
  readonly totalSegments: number;
};

export type ResourceOpenCryptoJob = Pick<
  ResourceOpenJob,
  "linkId" | "signingKey" | "encryptionKey" | "sealed"
>;

export type ResourceOpenHashPlan =
  | Tag<"OpenedStream", { readonly salt: OwnedBytes }>
  | Tag<"AfterDecompression">;

export type ResourceDigests = {
  readonly hash: OwnedBytes;
  readonly proof: OwnedBytes;
};

export type ResourceDigestLanding =
  | Tag<"Applied">
  | Tag<"Collision">
  | Tag<"Stale">
  | Tag<"Invalid">;

export type ResourceDigestExecution = Tag<"PortableWasm"> | Tag<"WebCrypto">;

export type ResourceOpenOutcome =
  | Tag<"Opened", OwnedBytes>
  | Tag<"Refused">;

type ImportedResourceKeys = {
  readonly signing: CryptoKey;
  readonly encryption: CryptoKey;
};

const RESOURCE_KEY_CACHE_CAPACITY = 64;
const RESOURCE_NONCE_BYTES = 4;
const WEB_CRYPTO_RESOURCE_DIGEST_MIN_BYTES = 768 * 1_024;

export function resourceDigestExecution(
  noncePrefixedByteLength: number,
  totalSegments: number,
): ResourceDigestExecution {
  const streamByteLength = noncePrefixedByteLength - RESOURCE_NONCE_BYTES;
  return streamByteLength >= WEB_CRYPTO_RESOURCE_DIGEST_MIN_BYTES || totalSegments >= 3
    ? Tag("WebCrypto")
    : Tag("PortableWasm");
}

export class WebCryptoResourceDigester {
  async digest(
    noncePrefixedPlaintext: Uint8Array,
    salt: Uint8Array,
  ): Promise<ResourceDigests> {
    if (salt.length !== RESOURCE_NONCE_BYTES) {
      throw new TypeError("resource digest salt must be exactly 4 bytes");
    }
    if (noncePrefixedPlaintext.length < RESOURCE_NONCE_BYTES) {
      throw new TypeError("resource digest plaintext must include its 4-byte nonce");
    }
    const stream = noncePrefixedPlaintext.subarray(RESOURCE_NONCE_BYTES);
    const input = new Uint8Array(stream.length + 32);
    input.set(stream);
    input.set(salt, stream.length);
    const hash = new Uint8Array(await crypto.subtle.digest(
      "SHA-256",
      input.subarray(0, stream.length + salt.length),
    )) as OwnedBytes;
    input.set(hash, stream.length);
    const proof = new Uint8Array(await crypto.subtle.digest("SHA-256", input)) as OwnedBytes;
    return { hash, proof };
  }
}

export class WebCryptoResourceSealer {
  readonly #keys = new Map<string, Promise<ImportedResourceKeys>>();

  async seal(job: ResourceSealCryptoJob): Promise<OwnedBytes> {
    const keys = await this.#keysFor(job);
    const encrypted = new Uint8Array(await crypto.subtle.encrypt(
      { name: "AES-CBC", iv: job.sealIv },
      keys.encryption,
      job.plaintext,
    ));
    const token = new Uint8Array(16 + encrypted.length + 32);
    token.set(job.sealIv);
    token.set(encrypted, 16);
    const tag = new Uint8Array(await crypto.subtle.sign(
      "HMAC",
      keys.signing,
      token.subarray(0, token.length - 32),
    ));
    token.set(tag, token.length - 32);
    return token as OwnedBytes;
  }

  #keysFor(job: ResourceSealCryptoJob): Promise<ImportedResourceKeys> {
    const key = byteKey(job.linkId);
    const existing = this.#keys.get(key);
    if (existing !== undefined) {
      return existing;
    }
    const imported = Promise.all([
      crypto.subtle.importKey(
        "raw",
        job.signingKey,
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["sign"],
      ),
      crypto.subtle.importKey(
        "raw",
        job.encryptionKey,
        "AES-CBC",
        false,
        ["encrypt"],
      ),
    ]).then(([signing, encryption]) => ({ signing, encryption }));
    rememberImportedKeys(this.#keys, key, imported);
    return imported;
  }
}

export class WebCryptoResourceOpener {
  readonly #keys = new Map<string, Promise<ImportedResourceKeys>>();

  async open(job: ResourceOpenCryptoJob): Promise<ResourceOpenOutcome> {
    if (job.sealed.length < 64 || (job.sealed.length - 48) % 16 !== 0) {
      return Tag("Refused");
    }
    const keys = await this.#keysFor(job);
    const signed = job.sealed.subarray(0, job.sealed.length - 32);
    const tag = job.sealed.subarray(job.sealed.length - 32);
    const authentic = await crypto.subtle.verify("HMAC", keys.signing, tag, signed);
    if (!authentic) {
      return Tag("Refused");
    }
    try {
      const plaintext = new Uint8Array(await crypto.subtle.decrypt(
        { name: "AES-CBC", iv: job.sealed.subarray(0, 16) },
        keys.encryption,
        job.sealed.subarray(16, job.sealed.length - 32),
      ));
      return Tag("Opened", plaintext);
    } catch {
      return Tag("Refused");
    }
  }

  #keysFor(job: ResourceOpenCryptoJob): Promise<ImportedResourceKeys> {
    const key = byteKey(job.linkId);
    const existing = this.#keys.get(key);
    if (existing !== undefined) {
      return existing;
    }
    const imported = Promise.all([
      crypto.subtle.importKey(
        "raw",
        job.signingKey,
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["verify"],
      ),
      crypto.subtle.importKey(
        "raw",
        job.encryptionKey,
        "AES-CBC",
        false,
        ["decrypt"],
      ),
    ]).then(([signing, encryption]) => ({ signing, encryption }));
    rememberImportedKeys(this.#keys, key, imported);
    return imported;
  }
}

export function parseResourceSealBegin(raw: unknown): ResourceSealBegin {
  const root = record(raw, "resource seal begin");
  const tag = stringField(root, "tag");
  const data = record(root.data, "resource seal begin data");
  if (tag === "Inline") {
    return Tag("Inline", { commandId: bigintField(data, "commandId") });
  }
  if (tag !== "Seal") {
    throw new TypeError(`unknown resource seal begin tag ${tag}`);
  }
  const job: ResourceSealJob = {
    commandId: bigintField(data, "commandId"),
    linkId: bytesField(data, "linkId", 16),
    streamNonce: bytesField(data, "streamNonce", 4),
    noncePrefixedBytes: positiveIntegerField(data, "noncePrefixedBytes"),
    totalSegments: positiveIntegerField(data, "totalSegments"),
    plaintext: bytesField(data, "plaintext"),
    signingKey: bytesField(data, "signingKey", 32),
    encryptionKey: bytesField(data, "encryptionKey", 32),
    sealIv: bytesField(data, "sealIv", 16),
    salts: bytesField(data, "salts", 32),
    promotionEntropy: bytesField(data, "promotionEntropy", 16),
  };
  if (job.plaintext.length !== job.noncePrefixedBytes) {
    throw new TypeError("plaintext length must equal noncePrefixedBytes");
  }
  if (!job.streamNonce.every((byte, index) => byte === job.plaintext[index])) {
    throw new TypeError("streamNonce must prefix plaintext");
  }
  return Tag("Seal", job);
}

export function parseResourceOpenJob(raw: unknown): ResourceOpenJob | undefined {
  if (raw === undefined) {
    return undefined;
  }
  const job = record(raw, "resource open job");
  const hashPlan = record(job.hashPlan, "resource open hash plan");
  const hashPlanTag = stringField(hashPlan, "tag");
  const parsedHashPlan = hashPlanTag === "OpenedStream"
    ? Tag("OpenedStream", {
      salt: bytesField(record(hashPlan.data, "resource open hash plan data"), "salt", 4),
    })
    : hashPlanTag === "AfterDecompression"
    ? Tag("AfterDecompression")
    : undefined;
  if (parsedHashPlan === undefined) {
    throw new TypeError(`unknown resource open hash plan tag ${hashPlanTag}`);
  }
  return {
    linkId: bytesField(job, "linkId", 16),
    hash: bytesField(job, "hash", 32),
    signingKey: bytesField(job, "signingKey", 32),
    encryptionKey: bytesField(job, "encryptionKey", 32),
    sealed: bytesField(job, "sealed"),
    hashPlan: parsedHashPlan,
    totalSegments: positiveIntegerField(job, "totalSegments"),
  };
}

export function parseResourceDigestLanding(raw: unknown): ResourceDigestLanding {
  const tag = stringField(record(raw, "resource digest landing"), "tag");
  if (tag === "Applied" || tag === "Collision" || tag === "Stale" || tag === "Invalid") {
    return Tag(tag);
  }
  throw new TypeError(`unknown resource digest landing tag ${tag}`);
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null) {
    throw new TypeError(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function stringField(value: Record<string, unknown>, key: string): string {
  const field = value[key];
  if (typeof field !== "string") {
    throw new TypeError(`${key} must be a string`);
  }
  return field;
}

function bigintField(value: Record<string, unknown>, key: string): bigint {
  const field = value[key];
  if (typeof field !== "bigint") {
    throw new TypeError(`${key} must be a bigint`);
  }
  return field;
}

function positiveIntegerField(value: Record<string, unknown>, key: string): number {
  const field = value[key];
  if (typeof field !== "number" || !Number.isSafeInteger(field) || field <= 0) {
    throw new TypeError(`${key} must be a positive safe integer`);
  }
  return field;
}

function bytesField(
  value: Record<string, unknown>,
  key: string,
  length?: number,
): OwnedBytes {
  const field = value[key];
  if (
    !(field instanceof Uint8Array) ||
    !(field.buffer instanceof ArrayBuffer) ||
    (length !== undefined && field.length !== length)
  ) {
    throw new TypeError(
      length === undefined
        ? `${key} must be a Uint8Array`
        : `${key} must be a ${length}-byte Uint8Array`,
    );
  }
  return field as OwnedBytes;
}

function byteKey(bytes: Uint8Array): string {
  let key = "";
  for (const byte of bytes) {
    key += byte.toString(16).padStart(2, "0");
  }
  return key;
}

function rememberImportedKeys(
  cache: Map<string, Promise<ImportedResourceKeys>>,
  key: string,
  imported: Promise<ImportedResourceKeys>,
): void {
  if (cache.size >= RESOURCE_KEY_CACHE_CAPACITY) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) {
      cache.delete(oldest);
    }
  }
  cache.set(key, imported);
}

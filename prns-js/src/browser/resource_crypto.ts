import { Tag } from "../casework.js";

export type ResourceCryptoExecution = Tag<"PortableWasm"> | Tag<"WebCrypto">;

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type ResourceSealJob = {
  readonly commandId: bigint;
  readonly linkId: OwnedBytes;
  readonly streamNonce: OwnedBytes;
  readonly noncePrefixedBytes: number;
  readonly plaintext: OwnedBytes;
  readonly signingKey: OwnedBytes;
  readonly encryptionKey: OwnedBytes;
  readonly sealIv: OwnedBytes;
  readonly salts: OwnedBytes;
  readonly promotionEntropy: OwnedBytes;
};

export type ResourceSealBegin =
  | Tag<"Inline", { readonly commandId: bigint }>
  | Tag<"Seal", ResourceSealJob>;

export type ResourceOpenJob = {
  readonly linkId: OwnedBytes;
  readonly hash: OwnedBytes;
  readonly signingKey: OwnedBytes;
  readonly encryptionKey: OwnedBytes;
  readonly sealed: OwnedBytes;
};

export type ResourceOpenOutcome =
  | Tag<"Opened", OwnedBytes>
  | Tag<"Refused">;

type ImportedResourceKeys = {
  readonly signing: CryptoKey;
  readonly encryption: CryptoKey;
};

const RESOURCE_KEY_CACHE_CAPACITY = 64;

export class WebCryptoResourceSealer {
  readonly #keys = new Map<string, Promise<ImportedResourceKeys>>();

  async seal(job: ResourceSealJob): Promise<Uint8Array> {
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
    return token;
  }

  #keysFor(job: ResourceSealJob): Promise<ImportedResourceKeys> {
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

  async open(job: ResourceOpenJob): Promise<ResourceOpenOutcome> {
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

  #keysFor(job: ResourceOpenJob): Promise<ImportedResourceKeys> {
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
  return {
    linkId: bytesField(job, "linkId", 16),
    hash: bytesField(job, "hash", 32),
    signingKey: bytesField(job, "signingKey", 32),
    encryptionKey: bytesField(job, "encryptionKey", 32),
    sealed: bytesField(job, "sealed"),
  };
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

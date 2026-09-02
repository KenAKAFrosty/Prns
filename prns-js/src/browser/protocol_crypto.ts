import { Tag } from "../casework.js";
import type { Tag as Tagged } from "../casework.js";
import { byteKey } from "./bytes.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type SignatureVerification = Tagged<"Valid"> | Tagged<"Invalid">;

export type WebCryptoPrimitiveCompatibility =
  | Tagged<"Compatible">
  | Tagged<"Unavailable", { readonly detail: string }>;

export type PrnsWebCryptoCompatibility = {
  readonly ed25519Sign: WebCryptoPrimitiveCompatibility;
  readonly ed25519Verify: WebCryptoPrimitiveCompatibility;
  readonly x25519: WebCryptoPrimitiveCompatibility;
  readonly hkdfSha256: WebCryptoPrimitiveCompatibility;
};

export type HkdfSha256Job = {
  readonly inputKeyMaterial: Uint8Array;
  readonly salt: Uint8Array;
  readonly info: Uint8Array;
  readonly outputBytes: number;
};

const ED25519_KEY_BYTES = 32;
const ED25519_SIGNATURE_BYTES = 64;
const X25519_KEY_BYTES = 32;
const SHA256_BYTES = 32;
const HKDF_SHA256_MAXIMUM_BYTES = 255 * SHA256_BYTES;
const PROTOCOL_PUBLIC_KEY_CACHE_CAPACITY = 64;
const ED25519_PKCS8_PREFIX = Uint8Array.of(
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06,
  0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
);
const X25519_PKCS8_PREFIX = Uint8Array.of(
  0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06,
  0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20,
);
const GOLDEN_ED25519_SECRET = new Uint8Array(ED25519_KEY_BYTES).fill(0x11);
const GOLDEN_ED25519_PUBLIC = bytesFromHex(
  "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
);
const GOLDEN_ED25519_MESSAGE = new TextEncoder().encode("sign-this");
const GOLDEN_ED25519_SIGNATURE = bytesFromHex(
  "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
  "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
);
const GOLDEN_X25519_SECRET = new Uint8Array(X25519_KEY_BYTES).fill(0x22);
const GOLDEN_X25519_PEER = bytesFromHex(
  "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14",
);
const GOLDEN_X25519_SHARED = bytesFromHex(
  "1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805",
);
const GOLDEN_HKDF_IKM = new Uint8Array(32).fill(0x42);
const GOLDEN_HKDF_SALT = new Uint8Array(16).fill(0x01);
const GOLDEN_HKDF_INFO = new TextEncoder().encode("context");
const GOLDEN_HKDF_OUTPUT = bytesFromHex(
  "d3a68f6569700c188c5a7c2bcd22c37e9757d022658f06b59753f7c079dcdb3a" +
  "82958b17892dbd30978719b5ba66787152ad0a0c7aeb4df49bce91d36c8915dd",
);

export class WebCryptoEd25519Signer {
  async sign(secretSeed: Uint8Array, message: Uint8Array): Promise<OwnedBytes> {
    exactLength(secretSeed, ED25519_KEY_BYTES, "Ed25519 secret seed");
    const encoded = privateKeyInfo(ED25519_PKCS8_PREFIX, secretSeed);
    try {
      const key = await crypto.subtle.importKey(
        "pkcs8",
        encoded,
        "Ed25519",
        false,
        ["sign"],
      );
      return new Uint8Array(await crypto.subtle.sign("Ed25519", key, ownedBytes(message)));
    } finally {
      encoded.fill(0);
    }
  }
}

export class WebCryptoEd25519Verifier {
  readonly #keys = new Map<string, Promise<CryptoKey>>();

  async verify(
    publicKey: Uint8Array,
    message: Uint8Array,
    signature: Uint8Array,
  ): Promise<SignatureVerification> {
    exactLength(publicKey, ED25519_KEY_BYTES, "Ed25519 public key");
    exactLength(signature, ED25519_SIGNATURE_BYTES, "Ed25519 signature");
    const key = await this.#keyFor(publicKey);
    const valid = await crypto.subtle.verify(
      "Ed25519",
      key,
      ownedBytes(signature),
      ownedBytes(message),
    );
    return valid ? Tag("Valid") : Tag("Invalid");
  }

  #keyFor(publicKey: Uint8Array): Promise<CryptoKey> {
    const id = byteKey(publicKey);
    const existing = this.#keys.get(id);
    if (existing !== undefined) {
      this.#keys.delete(id);
      this.#keys.set(id, existing);
      return existing;
    }
    const imported = crypto.subtle.importKey(
      "raw",
      ownedBytes(publicKey),
      "Ed25519",
      false,
      ["verify"],
    );
    rememberImportedKey(this.#keys, id, imported);
    return imported;
  }
}

export class WebCryptoX25519Deriver {
  readonly #peerKeys = new Map<string, Promise<CryptoKey>>();

  async derive(
    secretScalar: Uint8Array,
    peerPublicKey: Uint8Array,
  ): Promise<OwnedBytes> {
    exactLength(secretScalar, X25519_KEY_BYTES, "X25519 secret scalar");
    exactLength(peerPublicKey, X25519_KEY_BYTES, "X25519 public key");
    const encoded = privateKeyInfo(X25519_PKCS8_PREFIX, secretScalar);
    try {
      const [secret, peer] = await Promise.all([
        crypto.subtle.importKey("pkcs8", encoded, "X25519", false, ["deriveBits"]),
        this.#peerKeyFor(peerPublicKey),
      ]);
      return new Uint8Array(await crypto.subtle.deriveBits(
        { name: "X25519", public: peer },
        secret,
        X25519_KEY_BYTES * 8,
      ));
    } finally {
      encoded.fill(0);
    }
  }

  #peerKeyFor(publicKey: Uint8Array): Promise<CryptoKey> {
    const id = byteKey(publicKey);
    const existing = this.#peerKeys.get(id);
    if (existing !== undefined) {
      this.#peerKeys.delete(id);
      this.#peerKeys.set(id, existing);
      return existing;
    }
    const imported = crypto.subtle.importKey(
      "raw",
      ownedBytes(publicKey),
      "X25519",
      false,
      [],
    );
    rememberImportedKey(this.#peerKeys, id, imported);
    return imported;
  }
}

export class WebCryptoHkdfSha256Deriver {
  async derive(job: HkdfSha256Job): Promise<OwnedBytes> {
    if (
      !Number.isSafeInteger(job.outputBytes) ||
      job.outputBytes < 1 ||
      job.outputBytes > HKDF_SHA256_MAXIMUM_BYTES
    ) {
      throw new RangeError(
        `HKDF-SHA256 output must be between 1 and ${HKDF_SHA256_MAXIMUM_BYTES} bytes`,
      );
    }
    const key = await crypto.subtle.importKey(
      "raw",
      ownedBytes(job.inputKeyMaterial),
      "HKDF",
      false,
      ["deriveBits"],
    );
    return new Uint8Array(await crypto.subtle.deriveBits(
      {
        name: "HKDF",
        hash: "SHA-256",
        salt: ownedBytes(job.salt),
        info: ownedBytes(job.info),
      },
      key,
      job.outputBytes * 8,
    ));
  }
}

export async function verifyPrnsWebCryptoCompatibility(): Promise<PrnsWebCryptoCompatibility> {
  const [ed25519Sign, ed25519Verify, x25519, hkdfSha256] = await Promise.all([
    compatible(async () => {
      const signature = await new WebCryptoEd25519Signer().sign(
        GOLDEN_ED25519_SECRET,
        GOLDEN_ED25519_MESSAGE,
      );
      requireEqualBytes(signature, GOLDEN_ED25519_SIGNATURE, "Ed25519 signature");
    }),
    compatible(async () => {
      const verifier = new WebCryptoEd25519Verifier();
      const valid = await verifier.verify(
        GOLDEN_ED25519_PUBLIC,
        GOLDEN_ED25519_MESSAGE,
        GOLDEN_ED25519_SIGNATURE,
      );
      if (valid.tag !== "Valid") {
        throw new Error("Ed25519 golden signature was refused");
      }
      const invalid = await verifier.verify(
        GOLDEN_ED25519_PUBLIC,
        Uint8Array.of(...GOLDEN_ED25519_MESSAGE, 0),
        GOLDEN_ED25519_SIGNATURE,
      );
      if (invalid.tag !== "Invalid") {
        throw new Error("Ed25519 altered message was accepted");
      }
    }),
    compatible(async () => {
      const shared = await new WebCryptoX25519Deriver().derive(
        GOLDEN_X25519_SECRET,
        GOLDEN_X25519_PEER,
      );
      requireEqualBytes(shared, GOLDEN_X25519_SHARED, "X25519 shared secret");
    }),
    compatible(async () => {
      const output = await new WebCryptoHkdfSha256Deriver().derive({
        inputKeyMaterial: GOLDEN_HKDF_IKM,
        salt: GOLDEN_HKDF_SALT,
        info: GOLDEN_HKDF_INFO,
        outputBytes: GOLDEN_HKDF_OUTPUT.length,
      });
      requireEqualBytes(output, GOLDEN_HKDF_OUTPUT, "HKDF-SHA256 output");
    }),
  ]);
  return { ed25519Sign, ed25519Verify, x25519, hkdfSha256 };
}

async function compatible(operation: () => Promise<void>): Promise<WebCryptoPrimitiveCompatibility> {
  try {
    await operation();
    return Tag("Compatible");
  } catch (error) {
    return Tag("Unavailable", {
      detail: error instanceof Error ? error.message : String(error),
    });
  }
}

function privateKeyInfo(prefix: Uint8Array, secret: Uint8Array): OwnedBytes {
  const encoded = new Uint8Array(prefix.length + secret.length);
  encoded.set(prefix);
  encoded.set(secret, prefix.length);
  return encoded;
}

function exactLength(bytes: Uint8Array, length: number, name: string): void {
  if (bytes.length !== length) {
    throw new TypeError(`${name} must be exactly ${length} bytes`);
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

function bytesFromHex(value: string): OwnedBytes {
  const bytes = new Uint8Array(value.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

function requireEqualBytes(actual: Uint8Array, expected: Uint8Array, name: string): void {
  if (
    actual.length !== expected.length ||
    !actual.every((byte, index) => byte === expected[index])
  ) {
    throw new Error(`${name} did not match the Prns RNS 1.4.2 vector`);
  }
}

function rememberImportedKey(
  cache: Map<string, Promise<CryptoKey>>,
  id: string,
  imported: Promise<CryptoKey>,
): void {
  if (cache.size >= PROTOCOL_PUBLIC_KEY_CACHE_CAPACITY) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) {
      cache.delete(oldest);
    }
  }
  cache.set(id, imported);
  void imported.catch(() => {
    if (cache.get(id) === imported) {
      cache.delete(id);
    }
  });
}

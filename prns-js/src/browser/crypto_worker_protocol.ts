import type { Tag } from "../casework.js";
import type {
  PrnsWebCryptoCompatibility,
} from "./protocol_crypto.js";
import type {
  ResourceOpenCryptoJob,
  ResourceSealCryptoJob,
} from "./resource_crypto.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type CryptoWorkerRequest =
  | Tag<
      "Seal",
      {
        readonly id: number;
        readonly job: ResourceSealCryptoJob;
      }
    >
  | Tag<
      "Open",
      {
        readonly id: number;
        readonly job: ResourceOpenCryptoJob;
      }
    >
  | Tag<
      "Digest",
      {
        readonly id: number;
        readonly plaintext: OwnedBytes;
        readonly salt: OwnedBytes;
      }
    >
  | Tag<
      "Ed25519Sign",
      {
        readonly id: number;
        readonly secretSeed: OwnedBytes;
        readonly message: OwnedBytes;
      }
    >
  | Tag<
      "Ed25519Verify",
      {
        readonly id: number;
        readonly publicKey: OwnedBytes;
        readonly message: OwnedBytes;
        readonly signature: OwnedBytes;
      }
    >
  | Tag<
      "X25519Derive",
      {
        readonly id: number;
        readonly secretScalar: OwnedBytes;
        readonly peerPublicKey: OwnedBytes;
      }
    >
  | Tag<
      "LinkProofVerify",
      {
        readonly id: number;
        readonly publicKey: OwnedBytes;
        readonly message: OwnedBytes;
        readonly signature: OwnedBytes;
        readonly secretScalar: OwnedBytes;
        readonly peerPublicKey: OwnedBytes;
      }
    >
  | Tag<
      "HkdfSha256Derive",
      {
        readonly id: number;
        readonly inputKeyMaterial: OwnedBytes;
        readonly salt: OwnedBytes;
        readonly info: OwnedBytes;
        readonly outputBytes: number;
      }
    >;

export type CryptoWorkerResponse =
  | Tag<"Ready", { readonly compatibility: PrnsWebCryptoCompatibility }>
  | Tag<
      "Sealed",
      {
        readonly id: number;
        readonly sealed: ArrayBuffer;
        readonly plaintext: ArrayBuffer;
      }
    >
  | Tag<
      "Opened",
      {
        readonly id: number;
        readonly plaintext: ArrayBuffer;
      }
    >
  | Tag<"Refused", { readonly id: number }>
  | Tag<
      "Digested",
      {
        readonly id: number;
        readonly plaintext: ArrayBuffer;
        readonly hash: ArrayBuffer;
        readonly proof: ArrayBuffer;
      }
    >
  | Tag<
      "Ed25519Signed",
      {
        readonly id: number;
        readonly signature: ArrayBuffer;
      }
    >
  | Tag<"Ed25519Valid", { readonly id: number }>
  | Tag<"Ed25519Invalid", { readonly id: number }>
  | Tag<
      "X25519Derived",
      {
        readonly id: number;
        readonly sharedSecret: ArrayBuffer;
      }
    >
  | Tag<
      "LinkProofVerified",
      {
        readonly id: number;
        readonly sharedSecret: ArrayBuffer;
      }
    >
  | Tag<"LinkProofInvalid", { readonly id: number }>
  | Tag<
      "HkdfSha256Derived",
      {
        readonly id: number;
        readonly keyMaterial: ArrayBuffer;
      }
    >
  | Tag<
      "Failed",
      {
        readonly id: number;
        readonly detail: string;
      }
    >;

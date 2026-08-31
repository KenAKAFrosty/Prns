import type { Tag } from "../casework.js";
import type {
  ResourceOpenCryptoJob,
  ResourceSealCryptoJob,
} from "./resource_crypto.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type ResourceCryptoWorkerRequest =
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
    >;

export type ResourceCryptoWorkerResponse =
  | Tag<"Ready">
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
      "Failed",
      {
        readonly id: number;
        readonly detail: string;
      }
    >;

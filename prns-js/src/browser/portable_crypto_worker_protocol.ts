import type { Tag } from "../casework.js";

type OwnedBytes = Uint8Array<ArrayBuffer>;

export type PortableCryptoWorkerJob =
  | Tag<
      "AnnounceVerify",
      {
        readonly id: number;
        readonly publicKey: OwnedBytes;
        readonly message: OwnedBytes;
        readonly signature: OwnedBytes;
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
    >;

export type PortableCryptoWorkerRequest =
  | Tag<"Initialize", { readonly wasmModuleUrl?: string }>
  | Tag<"Perform", { readonly jobs: readonly PortableCryptoWorkerJob[] }>;

export type PortableCryptoWorkerOutcome =
  | Tag<"AnnounceValid", { readonly id: number }>
  | Tag<"AnnounceInvalid", { readonly id: number }>
  | Tag<
      "LinkProofVerified",
      { readonly id: number; readonly sharedSecret: OwnedBytes }
    >
  | Tag<"LinkProofInvalid", { readonly id: number }>
  | Tag<"OperationFailed", { readonly id: number; readonly detail: string }>;

export type PortableCryptoWorkerResponse =
  | Tag<"Ready">
  | Tag<"Settled", { readonly outcomes: readonly PortableCryptoWorkerOutcome[] }>
  | Tag<"InitializationFailed", { readonly detail: string }>;

export type PortableCryptoWasmModule = {
  portableEd25519Verify(
    publicKey: Uint8Array,
    message: Uint8Array,
    signature: Uint8Array,
  ): boolean;
  portableLinkProofVerify(
    publicKey: Uint8Array,
    message: Uint8Array,
    signature: Uint8Array,
    secretScalar: Uint8Array,
    peerPublicKey: Uint8Array,
  ): Uint8Array | undefined;
};

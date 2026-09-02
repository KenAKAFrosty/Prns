import { Tag, match } from "../casework.js";
import { loadBundledWasm, loadWasmModule } from "./bootstrap.js";
import type {
  PortableCryptoWasmModule,
  PortableCryptoWorkerJob,
  PortableCryptoWorkerOutcome,
  PortableCryptoWorkerRequest,
  PortableCryptoWorkerResponse,
} from "./portable_crypto_worker_protocol.js";

type WorkerScope = {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<PortableCryptoWorkerRequest>) => void,
  ): void;
  postMessage(message: PortableCryptoWorkerResponse, transfer: Transferable[]): void;
};

const ED25519_PUBLIC = bytesFromHex(
  "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
);
const ED25519_MESSAGE = new TextEncoder().encode("sign-this");
const ED25519_SIGNATURE = bytesFromHex(
  "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
    "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
);
const X25519_SECRET = new Uint8Array(32).fill(0x22);
const X25519_PEER = bytesFromHex(
  "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14",
);
const X25519_SHARED = bytesFromHex(
  "1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805",
);

const scope = globalThis as unknown as WorkerScope;
let wasm: PortableCryptoWasmModule | undefined;
let initialized = false;

scope.addEventListener("message", (event) => {
  void match(event.data, {
    Initialize: async ({ wasmModuleUrl }) => {
      if (initialized) {
        initializationFailed("portable crypto Worker was initialized more than once");
        return;
      }
      initialized = true;
      const loaded = wasmModuleUrl === undefined
        ? await loadBundledWasm()
        : await loadWasmModule(new URL(wasmModuleUrl));
      if (loaded.tag !== "Loaded") {
        initializationFailed(loaded.data.detail);
        return;
      }
      try {
        const candidate = portableCryptoModule(loaded.data);
        verifyCompatibility(candidate);
        wasm = candidate;
        scope.postMessage(Tag("Ready"), []);
      } catch (error) {
        initializationFailed(describeError(error));
      }
    },
    Perform: ({ jobs }) => {
      const active = wasm;
      if (active === undefined) {
        initializationFailed("portable crypto Worker received work before initialization");
        return;
      }
      const outcomes = jobs.map((job) => perform(active, job));
      scope.postMessage(Tag("Settled", { outcomes }), []);
    },
  });
});

function perform(
  active: PortableCryptoWasmModule,
  job: PortableCryptoWorkerJob,
): PortableCryptoWorkerOutcome {
  return match(job, {
    AnnounceVerify: ({ id, publicKey, message, signature }) => {
      try {
        return active.portableEd25519Verify(publicKey, message, signature)
          ? Tag("AnnounceValid", { id })
          : Tag("AnnounceInvalid", { id });
      } catch (error) {
        return Tag("OperationFailed", { id, detail: describeError(error) });
      }
    },
    LinkProofVerify: ({
      id,
      publicKey,
      message,
      signature,
      secretScalar,
      peerPublicKey,
    }) => {
      try {
        const sharedSecret = active.portableLinkProofVerify(
          publicKey,
          message,
          signature,
          secretScalar,
          peerPublicKey,
        );
        if (sharedSecret === undefined) {
          return Tag("LinkProofInvalid", { id });
        }
        const owned = ownedBytes(sharedSecret);
        return Tag("LinkProofVerified", { id, sharedSecret: owned });
      } catch (error) {
        return Tag("OperationFailed", { id, detail: describeError(error) });
      } finally {
        secretScalar.fill(0);
      }
    },
  });
}

function portableCryptoModule(value: unknown): PortableCryptoWasmModule {
  if (typeof value !== "object" || value === null) {
    throw new TypeError("portable crypto WebAssembly module must be an object");
  }
  const module = value as Record<string, unknown>;
  if (typeof module.portableEd25519Verify !== "function") {
    throw new TypeError("portable crypto WebAssembly module has no Ed25519 verifier");
  }
  if (typeof module.portableLinkProofVerify !== "function") {
    throw new TypeError("portable crypto WebAssembly module has no link-proof verifier");
  }
  return value as PortableCryptoWasmModule;
}

function verifyCompatibility(candidate: PortableCryptoWasmModule): void {
  if (!candidate.portableEd25519Verify(
    ED25519_PUBLIC,
    ED25519_MESSAGE,
    ED25519_SIGNATURE,
  )) {
    throw new TypeError("portable crypto WebAssembly module rejected the Ed25519 vector");
  }
  if (candidate.portableEd25519Verify(
    ED25519_PUBLIC,
    Uint8Array.of(...ED25519_MESSAGE, 0),
    ED25519_SIGNATURE,
  )) {
    throw new TypeError("portable crypto WebAssembly module accepted an invalid Ed25519 vector");
  }
  const secret = X25519_SECRET.slice();
  try {
    const shared = candidate.portableLinkProofVerify(
      ED25519_PUBLIC,
      ED25519_MESSAGE,
      ED25519_SIGNATURE,
      secret,
      X25519_PEER,
    );
    if (shared === undefined || !equalBytes(shared, X25519_SHARED)) {
      throw new TypeError("portable crypto WebAssembly module failed the link-proof vector");
    }
  } finally {
    secret.fill(0);
  }
}

function initializationFailed(detail: string): void {
  scope.postMessage(Tag("InitializationFailed", { detail }), []);
}

function ownedBytes(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  if (
    bytes.buffer instanceof ArrayBuffer &&
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength
  ) {
    return bytes as Uint8Array<ArrayBuffer>;
  }
  return bytes.slice() as Uint8Array<ArrayBuffer>;
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) {
    return false;
  }
  let difference = 0;
  for (let index = 0; index < left.byteLength; index += 1) {
    difference |= left[index]! ^ right[index]!;
  }
  return difference === 0;
}

function bytesFromHex(value: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(
    { length: value.length / 2 },
    (_, index) => Number.parseInt(value.slice(index * 2, index * 2 + 2), 16),
  ) as Uint8Array<ArrayBuffer>;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

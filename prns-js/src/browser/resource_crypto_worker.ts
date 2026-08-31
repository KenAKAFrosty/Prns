import { Tag, match } from "../casework.js";
import {
  WebCryptoResourceDigester,
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
} from "./resource_crypto.js";
import type {
  ResourceCryptoWorkerRequest,
  ResourceCryptoWorkerResponse,
} from "./resource_crypto_worker_protocol.js";

type WorkerScope = {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<ResourceCryptoWorkerRequest>) => void,
  ): void;
  postMessage(
    message: ResourceCryptoWorkerResponse,
    transfer: Transferable[],
  ): void;
};

const scope = globalThis as unknown as WorkerScope;
const sealer = new WebCryptoResourceSealer();
const opener = new WebCryptoResourceOpener();
const digester = new WebCryptoResourceDigester();

scope.addEventListener("message", (event) => {
  void perform(event.data);
});
scope.postMessage(Tag("Ready"), []);

async function perform(request: ResourceCryptoWorkerRequest): Promise<void> {
  await match(request, {
    Seal: async ({ id, job }) => {
      try {
        const sealed = await sealer.seal(job);
        scope.postMessage(
          Tag("Sealed", {
            id,
            sealed: sealed.buffer,
            plaintext: job.plaintext.buffer,
          }),
          [sealed.buffer, job.plaintext.buffer],
        );
      } catch (error) {
        failed(id, error);
      }
    },
    Open: async ({ id, job }) => {
      try {
        const outcome = await opener.open(job);
        match(outcome, {
          Opened: (plaintext) => {
            scope.postMessage(
              Tag("Opened", { id, plaintext: plaintext.buffer }),
              [plaintext.buffer],
            );
          },
          Refused: () => {
            scope.postMessage(Tag("Refused", { id }), []);
          },
        });
      } catch (error) {
        failed(id, error);
      }
    },
    Digest: async ({ id, plaintext, salt }) => {
      try {
        const digests = await digester.digest(plaintext, salt);
        scope.postMessage(
          Tag("Digested", {
            id,
            plaintext: plaintext.buffer,
            hash: digests.hash.buffer,
            proof: digests.proof.buffer,
          }),
          [plaintext.buffer, digests.hash.buffer, digests.proof.buffer],
        );
      } catch (error) {
        failed(id, error);
      }
    },
  });
}

function failed(id: number, error: unknown): void {
  scope.postMessage(
    Tag("Failed", {
      id,
      detail: error instanceof Error ? error.message : String(error),
    }),
    [],
  );
}

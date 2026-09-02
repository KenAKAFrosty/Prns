import { Tag, match } from "../casework.js";
import {
  WebCryptoResourceDigester,
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
} from "./resource_crypto.js";
import {
  WebCryptoEd25519Signer,
  WebCryptoEd25519Verifier,
  WebCryptoHkdfSha256Deriver,
  WebCryptoX25519Deriver,
  verifyPrnsWebCryptoCompatibility,
} from "./protocol_crypto.js";
import type {
  CryptoWorkerRequest,
  CryptoWorkerResponse,
} from "./crypto_worker_protocol.js";

type WorkerScope = {
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<CryptoWorkerRequest>) => void,
  ): void;
  postMessage(
    message: CryptoWorkerResponse,
    transfer: Transferable[],
  ): void;
};

const scope = globalThis as unknown as WorkerScope;
const sealer = new WebCryptoResourceSealer();
const opener = new WebCryptoResourceOpener();
const digester = new WebCryptoResourceDigester();
const ed25519Signer = new WebCryptoEd25519Signer();
const ed25519Verifier = new WebCryptoEd25519Verifier();
const x25519Deriver = new WebCryptoX25519Deriver();
const hkdfSha256Deriver = new WebCryptoHkdfSha256Deriver();

scope.addEventListener("message", (event) => {
  void perform(event.data);
});
void verifyPrnsWebCryptoCompatibility().then((compatibility) => {
  scope.postMessage(Tag("Ready", { compatibility }), []);
});

async function perform(request: CryptoWorkerRequest): Promise<void> {
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
            scope.postMessage(Tag("OpenAndDigestRefused", { id }), []);
          },
        });
      } catch (error) {
        failed(id, error);
      }
    },
    SealAndDigest: async ({ id, job, salt }) => {
      try {
        const [sealed, digests] = await Promise.all([
          sealer.seal(job),
          digester.digest(job.plaintext, salt),
        ]);
        scope.postMessage(
          Tag("SealedAndDigested", {
            id,
            sealed: sealed.buffer,
            plaintext: job.plaintext.buffer,
            hash: digests.hash.buffer,
            proof: digests.proof.buffer,
          }),
          [sealed.buffer, job.plaintext.buffer, digests.hash.buffer, digests.proof.buffer],
        );
      } catch (error) {
        failed(id, error);
      }
    },
    OpenAndDigest: async ({ id, job, salt }) => {
      try {
        const outcome = await opener.open(job);
        await match(outcome, {
          Opened: async (plaintext) => {
            const digests = await digester.digest(plaintext, salt);
            scope.postMessage(
              Tag("OpenedAndDigested", {
                id,
                plaintext: plaintext.buffer,
                hash: digests.hash.buffer,
                proof: digests.proof.buffer,
              }),
              [plaintext.buffer, digests.hash.buffer, digests.proof.buffer],
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
    Ed25519Sign: async ({ id, secretSeed, message }) => {
      try {
        const signature = await ed25519Signer.sign(secretSeed, message);
        scope.postMessage(
          Tag("Ed25519Signed", { id, signature: signature.buffer }),
          [signature.buffer],
        );
      } catch (error) {
        failed(id, error);
      } finally {
        secretSeed.fill(0);
      }
    },
    Ed25519Verify: async ({ id, publicKey, message, signature }) => {
      try {
        const verification = await ed25519Verifier.verify(publicKey, message, signature);
        scope.postMessage(
          verification.tag === "Valid"
            ? Tag("Ed25519Valid", { id })
            : Tag("Ed25519Invalid", { id }),
          [],
        );
      } catch (error) {
        failed(id, error);
      }
    },
    X25519Derive: async ({ id, secretScalar, peerPublicKey }) => {
      try {
        const sharedSecret = await x25519Deriver.derive(secretScalar, peerPublicKey);
        scope.postMessage(
          Tag("X25519Derived", { id, sharedSecret: sharedSecret.buffer }),
          [sharedSecret.buffer],
        );
      } catch (error) {
        failed(id, error);
      } finally {
        secretScalar.fill(0);
      }
    },
    LinkProofVerify: async ({
      id,
      publicKey,
      message,
      signature,
      secretScalar,
      peerPublicKey,
    }) => {
      try {
        const verification = await ed25519Verifier.verify(publicKey, message, signature);
        if (verification.tag === "Invalid") {
          scope.postMessage(Tag("LinkProofInvalid", { id }), []);
          return;
        }
        const sharedSecret = await x25519Deriver.derive(secretScalar, peerPublicKey);
        scope.postMessage(
          Tag("LinkProofVerified", { id, sharedSecret: sharedSecret.buffer }),
          [sharedSecret.buffer],
        );
      } catch (error) {
        failed(id, error);
      } finally {
        secretScalar.fill(0);
      }
    },
    HkdfSha256Derive: async ({
      id,
      inputKeyMaterial,
      salt,
      info,
      outputBytes,
    }) => {
      try {
        const keyMaterial = await hkdfSha256Deriver.derive({
          inputKeyMaterial,
          salt,
          info,
          outputBytes,
        });
        scope.postMessage(
          Tag("HkdfSha256Derived", { id, keyMaterial: keyMaterial.buffer }),
          [keyMaterial.buffer],
        );
      } catch (error) {
        failed(id, error);
      } finally {
        inputKeyMaterial.fill(0);
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

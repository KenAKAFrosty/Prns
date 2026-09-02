import { Tag, match } from "../../dist/casework.js";

const MAXIMUM_PENDING_JOBS = 16;

export class ConcurrentWebCryptoGateway {
  #worker;
  #maximumInFlight;
  #nextId = 1;
  #queue = [];
  #inFlight = new Map();
  #maximumObservedInFlight = 0;
  #closed = false;
  #compatibility;
  #ready;
  #settleReady;

  constructor(maximumInFlight) {
    this.#maximumInFlight = maximumInFlight;
    this.#ready = new Promise((settle) => {
      this.#settleReady = settle;
    });
    this.#worker = new Worker("../../dist/browser/crypto_worker.js", {
      type: "module",
      name: `prns-webcrypto-gateway-${maximumInFlight}`,
    });
    this.#worker.addEventListener("message", ({ data }) => this.#receive(data));
    this.#worker.addEventListener("error", (event) => {
      this.#failAll(event.message || "WebCrypto gateway failed");
    });
    this.#worker.addEventListener("messageerror", () => {
      this.#failAll("WebCrypto gateway emitted an unreadable response");
    });
  }

  ready() {
    return this.#ready;
  }

  maximumObservedInFlight() {
    return this.#maximumObservedInFlight;
  }

  donateSeal(job) {
    const plaintext = transferableBytes(job.plaintext);
    const signingKey = transferableBytes(job.signingKey);
    const encryptionKey = transferableBytes(job.encryptionKey);
    return this.#submit(
      "Resource",
      (id) => Tag("Seal", {
        id,
        job: {
          linkId: job.linkId,
          plaintext,
          signingKey,
          encryptionKey,
          sealIv: job.sealIv,
        },
      }),
      uniqueTransfers([plaintext, signingKey, encryptionKey]),
    );
  }

  donateOpen(job) {
    const sealed = transferableBytes(job.sealed);
    const signingKey = transferableBytes(job.signingKey);
    const encryptionKey = transferableBytes(job.encryptionKey);
    return this.#submit(
      "Resource",
      (id) => Tag("Open", {
        id,
        job: {
          linkId: job.linkId,
          sealed,
          signingKey,
          encryptionKey,
        },
      }),
      uniqueTransfers([sealed, signingKey, encryptionKey]),
    );
  }

  donateDigest(plaintextBytes, saltBytes) {
    const plaintext = transferableBytes(plaintextBytes);
    const salt = transferableBytes(saltBytes);
    return this.#submit(
      "Resource",
      (id) => Tag("Digest", { id, plaintext, salt }),
      uniqueTransfers([plaintext]),
    );
  }

  donateEd25519Sign(secretSeedBytes, messageBytes) {
    const secretSeed = transferableBytes(secretSeedBytes);
    const message = transferableBytes(messageBytes);
    return this.#submit(
      "Protocol",
      (id) => Tag("Ed25519Sign", { id, secretSeed, message }),
      uniqueTransfers([secretSeed, message]),
    );
  }

  donateEd25519Verify(publicKeyBytes, messageBytes, signatureBytes) {
    const publicKey = transferableBytes(publicKeyBytes);
    const message = transferableBytes(messageBytes);
    const signature = transferableBytes(signatureBytes);
    return this.#submit(
      "Protocol",
      (id) => Tag("Ed25519Verify", { id, publicKey, message, signature }),
      uniqueTransfers([publicKey, message, signature]),
    );
  }

  donateX25519Derive(secretScalarBytes, peerPublicKeyBytes) {
    const secretScalar = transferableBytes(secretScalarBytes);
    const peerPublicKey = transferableBytes(peerPublicKeyBytes);
    return this.#submit(
      "Protocol",
      (id) => Tag("X25519Derive", { id, secretScalar, peerPublicKey }),
      uniqueTransfers([secretScalar, peerPublicKey]),
    );
  }

  donateHkdfSha256(job) {
    const inputKeyMaterial = transferableBytes(job.inputKeyMaterial);
    const salt = transferableBytes(job.salt);
    const info = transferableBytes(job.info);
    return this.#submit(
      "Protocol",
      (id) => Tag("HkdfSha256Derive", {
        id,
        inputKeyMaterial,
        salt,
        info,
        outputBytes: job.outputBytes,
      }),
      uniqueTransfers([inputKeyMaterial, salt, info]),
    );
  }

  close() {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#failAll("WebCrypto gateway closed");
    this.#worker.terminate();
  }

  #submit(priority, requestFor, transfer) {
    if (this.#closed || this.#queue.length + this.#inFlight.size >= MAXIMUM_PENDING_JOBS) {
      return Promise.resolve(Tag("Busy"));
    }
    const id = this.#nextId;
    this.#nextId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    return new Promise((settle) => {
      const pending = {
        id,
        priority,
        request: requestFor(id),
        transfer,
        settle,
      };
      const firstResource = this.#queue.findIndex((job) => job.priority === "Resource");
      if (priority === "Protocol" && firstResource >= 0) {
        this.#queue.splice(firstResource, 0, pending);
      } else {
        this.#queue.push(pending);
      }
      this.#dispatch();
    });
  }

  #dispatch() {
    if (this.#closed || this.#compatibility === undefined) {
      return;
    }
    while (
      this.#inFlight.size < this.#maximumInFlight &&
      this.#queue.length > 0
    ) {
      const pending = this.#queue.shift();
      this.#inFlight.set(pending.id, pending);
      this.#maximumObservedInFlight = Math.max(
        this.#maximumObservedInFlight,
        this.#inFlight.size,
      );
      try {
        this.#worker.postMessage(pending.request, pending.transfer);
      } catch (error) {
        this.#inFlight.delete(pending.id);
        pending.settle(failed(error));
      }
    }
  }

  #receive(response) {
    match(response, {
      Ready: ({ compatibility }) => {
        if (this.#compatibility !== undefined) {
          this.#failAll("WebCrypto gateway became ready twice");
          return;
        }
        this.#compatibility = compatibility;
        this.#settleReady(Tag("Ready", {
          workers: 1,
          maximumInFlight: this.#maximumInFlight,
          compatibility,
        }));
        this.#dispatch();
      },
      Sealed: ({ id, sealed, plaintext }) => {
        this.#complete(id, Tag("Sealed", {
          sealed: new Uint8Array(sealed),
          plaintext: new Uint8Array(plaintext),
        }));
      },
      Opened: ({ id, plaintext }) => {
        this.#complete(id, Tag("Opened", new Uint8Array(plaintext)));
      },
      Refused: ({ id }) => this.#complete(id, Tag("Refused")),
      Digested: ({ id, plaintext, hash, proof }) => {
        this.#complete(id, Tag("Digested", {
          plaintext: new Uint8Array(plaintext),
          hash: new Uint8Array(hash),
          proof: new Uint8Array(proof),
        }));
      },
      Ed25519Signed: ({ id, signature }) => {
        this.#complete(id, Tag("Signed", { signature: new Uint8Array(signature) }));
      },
      Ed25519Valid: ({ id }) => this.#complete(id, Tag("Valid")),
      Ed25519Invalid: ({ id }) => this.#complete(id, Tag("Invalid")),
      X25519Derived: ({ id, sharedSecret }) => {
        this.#complete(id, Tag("Derived", { sharedSecret: new Uint8Array(sharedSecret) }));
      },
      LinkProofVerified: ({ id, sharedSecret }) => {
        this.#complete(id, Tag("Verified", { sharedSecret: new Uint8Array(sharedSecret) }));
      },
      LinkProofInvalid: ({ id }) => this.#complete(id, Tag("Invalid")),
      HkdfSha256Derived: ({ id, keyMaterial }) => {
        this.#complete(id, Tag("Derived", { keyMaterial: new Uint8Array(keyMaterial) }));
      },
      Failed: ({ id, detail }) => this.#complete(id, Tag("Failed", { detail })),
    });
  }

  #complete(id, settlement) {
    const pending = this.#inFlight.get(id);
    if (pending === undefined) {
      this.#failAll("WebCrypto gateway settled an unknown job");
      return;
    }
    this.#inFlight.delete(id);
    pending.settle(settlement);
    this.#dispatch();
  }

  #failAll(detail) {
    this.#closed = true;
    const settlement = Tag("Failed", { detail });
    for (const pending of this.#queue.splice(0)) {
      pending.settle(settlement);
    }
    for (const pending of this.#inFlight.values()) {
      pending.settle(settlement);
    }
    this.#inFlight.clear();
    if (this.#compatibility === undefined) {
      this.#settleReady(Tag("Unavailable"));
    }
  }
}

function transferableBytes(bytes) {
  if (
    bytes.buffer instanceof ArrayBuffer &&
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength
  ) {
    return bytes;
  }
  return bytes.slice();
}

function uniqueTransfers(views) {
  return [...new Set(views.map((view) => view.buffer))];
}

function failed(error) {
  return Tag("Failed", {
    detail: error instanceof Error ? error.message : String(error),
  });
}

import assert from "node:assert/strict";
import test from "node:test";

import {
  WebCryptoEd25519Signer,
  WebCryptoEd25519Verifier,
  WebCryptoHkdfSha256Deriver,
  WebCryptoX25519Deriver,
  verifyPrnsWebCryptoCompatibility,
} from "../dist/browser/protocol_crypto.js";
import { parseProtocolCryptoJob } from "../dist/browser/protocol_crypto_runtime.js";

const ed25519Secret = new Uint8Array(32).fill(0x11);
const ed25519Public = bytesFromHex(
  "d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737",
);
const ed25519Message = new TextEncoder().encode("sign-this");
const ed25519Signature = bytesFromHex(
  "ee646fb3251af01efbe35f4b03905b3ec2b90ea4acd9a51a46cb795f76575b4a" +
  "36e2893c356db8b2135417f6001a99ecd81de04dde2f2b3428fd4f8ea46e1107",
);
const x25519Secret = new Uint8Array(32).fill(0x22);
const x25519Peer = bytesFromHex(
  "7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14",
);
const x25519Shared = bytesFromHex(
  "1fdc192faa0212a9aae7bb4f41b580227fd5ad3e5d777faae230dfe973f3e805",
);
const hkdfOutput = bytesFromHex(
  "d3a68f6569700c188c5a7c2bcd22c37e9757d022658f06b59753f7c079dcdb3a" +
  "82958b17892dbd30978719b5ba66787152ad0a0c7aeb4df49bce91d36c8915dd",
);

test("Web Crypto passes every Prns asymmetric and HKDF compatibility vector", async () => {
  assert.deepEqual(await verifyPrnsWebCryptoCompatibility(), {
    ed25519Sign: { tag: "Compatible", data: undefined },
    ed25519Verify: { tag: "Compatible", data: undefined },
    x25519: { tag: "Compatible", data: undefined },
    hkdfSha256: { tag: "Compatible", data: undefined },
  });
});

test("Web Crypto Ed25519 signs and verifies exact RNS 1.4.2 bytes", async () => {
  assert.deepEqual(
    await new WebCryptoEd25519Signer().sign(ed25519Secret, ed25519Message),
    ed25519Signature,
  );
  const verifier = new WebCryptoEd25519Verifier();
  assert.deepEqual(
    await verifier.verify(ed25519Public, ed25519Message, ed25519Signature),
    { tag: "Valid", data: undefined },
  );
  assert.deepEqual(
    await verifier.verify(
      ed25519Public,
      Uint8Array.of(...ed25519Message, 0),
      ed25519Signature,
    ),
    { tag: "Invalid", data: undefined },
  );
});

test("Web Crypto X25519 and HKDF reproduce exact RNS 1.4.2 bytes", async () => {
  assert.deepEqual(
    await new WebCryptoX25519Deriver().derive(x25519Secret, x25519Peer),
    x25519Shared,
  );
  assert.deepEqual(
    await new WebCryptoHkdfSha256Deriver().derive({
      inputKeyMaterial: new Uint8Array(32).fill(0x42),
      salt: new Uint8Array(16).fill(0x01),
      info: new TextEncoder().encode("context"),
      outputBytes: 64,
    }),
    hkdfOutput,
  );
});

test("Web Crypto protocol primitives reject malformed sizes before cryptography", async () => {
  await assert.rejects(
    new WebCryptoEd25519Signer().sign(new Uint8Array(31), ed25519Message),
    /secret seed must be exactly 32 bytes/,
  );
  await assert.rejects(
    new WebCryptoEd25519Verifier().verify(
      ed25519Public,
      ed25519Message,
      new Uint8Array(63),
    ),
    /signature must be exactly 64 bytes/,
  );
  await assert.rejects(
    new WebCryptoX25519Deriver().derive(x25519Secret, new Uint8Array(31)),
    /public key must be exactly 32 bytes/,
  );
  await assert.rejects(
    new WebCryptoHkdfSha256Deriver().derive({
      inputKeyMaterial: new Uint8Array(32),
      salt: new Uint8Array(),
      info: new Uint8Array(),
      outputBytes: 255 * 32 + 1,
    }),
    /output must be between 1 and 8160 bytes/,
  );
});

test("protocol crypto runtime jobs preserve their exact operation shapes", () => {
  assert.deepEqual(
    parseProtocolCryptoJob({
      tag: "AnnounceVerify",
      data: {
        id: 1,
        publicKey: ed25519Public,
        message: ed25519Message,
        signature: ed25519Signature,
      },
    }),
    {
      tag: "AnnounceVerify",
      data: {
        id: 1,
        publicKey: ed25519Public,
        message: ed25519Message,
        signature: ed25519Signature,
      },
    },
  );
  assert.deepEqual(
    parseProtocolCryptoJob({
      tag: "LinkProofVerify",
      data: {
        id: 2,
        publicKey: ed25519Public,
        message: ed25519Message,
        signature: ed25519Signature,
        secretScalar: x25519Secret,
        peerPublicKey: x25519Peer,
      },
    }),
    {
      tag: "LinkProofVerify",
      data: {
        id: 2,
        publicKey: ed25519Public,
        message: ed25519Message,
        signature: ed25519Signature,
        secretScalar: x25519Secret,
        peerPublicKey: x25519Peer,
      },
    },
  );
  assert.throws(
    () => parseProtocolCryptoJob({
      tag: "LinkProofVerify",
      data: {
        id: 3,
        publicKey: ed25519Public,
        message: ed25519Message,
        signature: ed25519Signature,
        secretScalar: new Uint8Array(31),
        peerPublicKey: x25519Peer,
      },
    }),
    /secretScalar must be exactly 32 bytes/,
  );
});

function bytesFromHex(value) {
  return Uint8Array.from(
    { length: value.length / 2 },
    (_, index) => Number.parseInt(value.slice(index * 2, index * 2 + 2), 16),
  );
}

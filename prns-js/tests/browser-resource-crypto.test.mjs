import assert from "node:assert/strict";
import test from "node:test";

import {
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
  parseResourceOpenJob,
  parseResourceSealBegin,
} from "../dist/browser/resource_crypto.js";

function sealJob() {
  return {
    commandId: 41n,
    linkId: Uint8Array.from({ length: 16 }, (_, index) => index),
    streamNonce: Uint8Array.of(1, 2, 3, 4),
    noncePrefixedBytes: 21,
    plaintext: Uint8Array.of(
      1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
    ),
    signingKey: Uint8Array.from({ length: 32 }, (_, index) => index + 1),
    encryptionKey: Uint8Array.from({ length: 32 }, (_, index) => 255 - index),
    sealIv: Uint8Array.from({ length: 16 }, (_, index) => index + 31),
    salts: new Uint8Array(32),
    promotionEntropy: new Uint8Array(16),
  };
}

test("resource seal parsing preserves the typed job and rejects inconsistent identity", () => {
  const job = sealJob();
  assert.deepEqual(
    parseResourceSealBegin({ tag: "Seal", data: job }),
    { tag: "Seal", data: job },
  );
  assert.throws(
    () => parseResourceSealBegin({
      tag: "Seal",
      data: { ...job, noncePrefixedBytes: job.noncePrefixedBytes + 1 },
    }),
    /plaintext length must equal noncePrefixedBytes/,
  );
  assert.throws(
    () => parseResourceSealBegin({
      tag: "Seal",
      data: { ...job, streamNonce: Uint8Array.of(9, 2, 3, 4) },
    }),
    /streamNonce must prefix plaintext/,
  );
});

test("Web Crypto resource tokens open exactly and refuse authentication failures", async () => {
  const job = sealJob();
  const sealed = await new WebCryptoResourceSealer().seal(job);
  const openJob = parseResourceOpenJob({
    linkId: job.linkId,
    hash: new Uint8Array(32),
    signingKey: job.signingKey,
    encryptionKey: job.encryptionKey,
    sealed,
  });
  assert.notEqual(openJob, undefined);
  assert.deepEqual(
    await new WebCryptoResourceOpener().open(openJob),
    { tag: "Opened", data: job.plaintext },
  );

  const tampered = sealed.slice();
  tampered[16] ^= 1;
  assert.deepEqual(
    await new WebCryptoResourceOpener().open({ ...openJob, sealed: tampered }),
    { tag: "Refused", data: undefined },
  );
});

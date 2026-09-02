import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  WebCryptoResourceDigester,
  WebCryptoResourceOpener,
  WebCryptoResourceSealer,
  parseResourceOpenJob,
  parseResourceSealJob,
  resourceDigestExecution,
  resourceOpenDigestExecution,
} from "../dist/browser/resource_crypto.js";
import {
  BrowserCryptoExecutor,
  WebCryptoWorkerPool,
} from "../dist/browser/crypto_pool.js";

function sealJob() {
  return {
    id: 41,
    linkId: Uint8Array.from({ length: 16 }, (_, index) => index),
    noncePrefixedBytes: 21,
    totalSegments: 1,
    plaintext: Uint8Array.of(
      1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
    ),
    signingKey: Uint8Array.from({ length: 32 }, (_, index) => index + 1),
    encryptionKey: Uint8Array.from({ length: 32 }, (_, index) => 255 - index),
    sealIv: Uint8Array.from({ length: 16 }, (_, index) => index + 31),
    salts: new Uint8Array(32),
  };
}

test("resource seal parsing preserves the typed job and rejects inconsistent shape", () => {
  const job = sealJob();
  assert.deepEqual(parseResourceSealJob(job), job);
  assert.throws(
    () => parseResourceSealJob({
      ...job,
      noncePrefixedBytes: job.noncePrefixedBytes + 1,
    }),
    /plaintext length must equal noncePrefixedBytes/,
  );
  assert.throws(
    () => parseResourceSealJob({ ...job, salts: new Uint8Array(28) }),
    /salts must be a 32-byte Uint8Array/,
  );
});

test("Web Crypto resource tokens open exactly and refuse authentication failures", async () => {
  const job = sealJob();
  const sealed = await new WebCryptoResourceSealer().seal(job);
  const openJob = parseResourceOpenJob({
    id: 42,
    linkId: job.linkId,
    hash: new Uint8Array(32),
    signingKey: job.signingKey,
    encryptionKey: job.encryptionKey,
    sealed,
    hashPlan: { tag: "OpenedStream", data: { salt: Uint8Array.of(9, 8, 7, 6) } },
    totalSegments: 1,
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

test("Web Crypto resource digests match independent SHA-256 hash and proof bytes", async () => {
  const stream = Uint8Array.from({ length: 257 }, (_, index) => index);
  const plaintext = new Uint8Array(stream.length + 4);
  plaintext.set(Uint8Array.of(1, 2, 3, 4));
  plaintext.set(stream, 4);
  const salt = Uint8Array.of(9, 8, 7, 6);
  const expectedHash = new Uint8Array(
    createHash("sha256").update(stream).update(salt).digest(),
  );
  const expectedProof = new Uint8Array(
    createHash("sha256").update(stream).update(expectedHash).digest(),
  );
  const digests = await new WebCryptoResourceDigester().digest(plaintext, salt);
  assert.deepEqual(digests.hash, expectedHash);
  assert.deepEqual(digests.proof, expectedProof);
});

test("resource digest execution keeps small isolated work local and offloads overlap", () => {
  const tokenOverheadBytes = 48;
  assert.deepEqual(resourceDigestExecution(512 * 1_024 + 4, 2), {
    tag: "PortableWasm",
    data: undefined,
  });
  assert.deepEqual(resourceDigestExecution(1_024 * 1_024 + 4, 1), {
    tag: "WebCrypto",
    data: undefined,
  });
  assert.deepEqual(resourceDigestExecution(512 * 1_024 + 4, 3), {
    tag: "WebCrypto",
    data: undefined,
  });
  assert.deepEqual(resourceOpenDigestExecution(512 * 1_024 + tokenOverheadBytes, 2), {
    tag: "PortableWasm",
    data: undefined,
  });
  assert.deepEqual(resourceOpenDigestExecution(512 * 1_024 + tokenOverheadBytes, 3), {
    tag: "WebCrypto",
    data: undefined,
  });
});

test("resource crypto executor preserves behavior when Workers are unavailable", async () => {
  const executor = new BrowserCryptoExecutor(2);
  const job = sealJob();
  const sealed = await executor.seal(job);
  assert.equal(sealed.tag, "Sealed");
  const digested = await executor.digest(
    sealed.data.plaintext,
    Uint8Array.of(9, 8, 7, 6),
  );
  assert.equal(digested.tag, "Digested");
  const sealedAndDigested = await executor.sealAndDigest(
    job,
    Uint8Array.of(9, 8, 7, 6),
  );
  assert.equal(sealedAndDigested.tag, "SealedAndDigested");
  const opened = await executor.open({
    linkId: job.linkId,
    signingKey: job.signingKey,
    encryptionKey: job.encryptionKey,
    sealed: sealed.data.sealed,
  });
  assert.deepEqual(opened, { tag: "Opened", data: job.plaintext });
  const openedAndDigested = await executor.openAndDigest(
    {
      linkId: job.linkId,
      signingKey: job.signingKey,
      encryptionKey: job.encryptionKey,
      sealed: sealedAndDigested.data.sealed,
    },
    Uint8Array.of(9, 8, 7, 6),
  );
  assert.deepEqual(openedAndDigested, {
    tag: "OpenedAndDigested",
    data: {
      plaintext: job.plaintext,
      hash: sealedAndDigested.data.hash,
      proof: sealedAndDigested.data.proof,
    },
  });
  executor.close();
  assert.deepEqual(await executor.seal(job), {
    tag: "Failed",
    data: { detail: "crypto executor is closed" },
  });
});

test("resource crypto pool rejects invalid worker counts", async () => {
  assert.throws(() => new WebCryptoWorkerPool(0), /crypto workers must be between/);
  assert.throws(() => new WebCryptoWorkerPool(17), /crypto workers must be between/);
  const unavailable = new WebCryptoWorkerPool(1);
  assert.deepEqual(await unavailable.ready(), { tag: "Unavailable", data: undefined });
  unavailable.close();
});

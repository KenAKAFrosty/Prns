import assert from "node:assert/strict";
import { beforeEach, mock, test } from "node:test";
import { setImmediate as nextTurn } from "node:timers/promises";

// Mock only platform/native boundaries; exercise the actual product TypeScript.
let stored;
let calls;
let read;
let write;
let remove;
class Authority extends Error {
  tag = "Authority";
  constructor(reason) { super("native authority error"); this.inner = { reason }; }
  static instanceOf(value) { return value.tag === "Authority"; }
}
const invalidMetadata = 3;
const core = {
  IdentityKeySource: { Generate: class Generate {} },
  IdentityApiError: { Authority },
  IdentityFault: { InvalidMetadata: invalidMetadata },
};
mock.module("@react-native-async-storage/async-storage", { exports: { default: {
  getItem: async (key) => { calls.push(["read", key]); return read(); },
  setItem: async (key, value) => { calls.push(["write", key, value]); return write(value); },
  removeItem: async (key) => { calls.push(["remove", key]); return remove(); },
} } });
mock.module("expo-crypto", { exports: {
  getRandomBytesAsync: async (length) => new Uint8Array(length).fill(0xab),
} });
mock.module("@prns/app-bindings", { exports: { prns_identities_core: core } });
const {
  CreationDraftConflict, CreationMetadataRejected, decodeCreationDraft, hex,
  loadCreationDraft, prepareCreationDraft, submitCreationDraft,
} = await import("../src/identity-creation.ts");

beforeEach(() => {
  stored = null;
  calls = [];
  read = async () => stored;
  write = async (value) => { stored = value; };
  remove = async () => { stored = null; };
});
function draft(nonce = "ab", label = "Personal") {
  return Object.freeze({ version: 2, incarnation: "12".repeat(16), epoch: "18446744073709551615",
    nonce: nonce.repeat(32), localLabel: label, publishedName: "Pat" });
}
function catalog(create = async () => undefined) {
  return {
    creationScope: async () => ({ incarnation: new Uint8Array(16).fill(0x12).buffer,
      epoch: 0xffffffffffffffffn }),
    createOrRecover: async (...args) => { calls.push(["native", ...args]); return create(...args); },
  };
}
const nativeCalls = () => calls.filter(([kind]) => kind === "native");

test("record roundtrip keeps exact scoped bytes, bigint and immutable form input", async () => {
  const metadata = { localLabel: "Personal", publishedName: "Pat" };
  const preparing = prepareCreationDraft(catalog(), metadata);
  metadata.localLabel = "changed after submission";
  const value = await preparing;
  assert.deepEqual(value, draft());
  assert.ok(Object.isFrozen(value));
  assert.deepEqual(decodeCreationDraft(JSON.stringify(value)), value);
  stored = JSON.stringify(value);
  assert.deepEqual(await loadCreationDraft(), value);
  await submitCreationDraft(catalog(), value);
  const [, id, intent, importKey] = nativeCalls()[0];
  assert.equal(hex(id.scope.incarnation), value.incarnation);
  assert.equal(id.scope.epoch, 0xffffffffffffffffn);
  assert.equal(hex(id.nonce), value.nonce);
  assert.deepEqual(intent.metadata, { localLabel: value.localLabel, publishedName: value.publishedName });
  assert.deepEqual(intent.services, []);
  assert.ok(intent.source instanceof core.IdentityKeySource.Generate);
  assert.equal(importKey, undefined);
  assert.equal(stored, null);
});

test("malformed saved input is preserved and cannot be silently replaced", async () => {
  for (const corrupt of ["broken json", JSON.stringify({ ...draft(), nonce: "00" }),
    JSON.stringify({ ...draft(), epoch: "18446744073709551616" })]) {
    stored = corrupt;
    await assert.rejects(loadCreationDraft());
    await assert.rejects(submitCreationDraft(catalog(), draft()));
    assert.equal(stored, corrupt);
  }
  assert.equal(nativeCalls().length, 0);
  assert.equal(calls.filter(([kind]) => kind === "write" || kind === "remove").length, 0);
});

test("native admission and other storage sections wait for the exact save", async () => {
  const writing = Promise.withResolvers();
  const written = Promise.withResolvers();
  write = async (value) => { writing.resolve(); await written.promise; stored = value; };
  const submitted = submitCreationDraft(catalog(), draft());
  await writing.promise;
  let loaded = false;
  const load = loadCreationDraft().then((value) => { loaded = true; return value; });
  await nextTurn();
  assert.equal(nativeCalls().length, 0);
  assert.equal(loaded, false);
  written.resolve();
  assert.deepEqual(await load, draft());
  await submitted;
  assert.equal(nativeCalls().length, 1);
});

test("uncertain save failure never submits and the exact ID can recover later", async () => {
  const failure = new Error("save reply lost");
  write = async (value) => { stored = value; throw failure; };
  await assert.rejects(submitCreationDraft(catalog(), draft()), (error) => error === failure);
  assert.equal(nativeCalls().length, 0);
  assert.deepEqual(await loadCreationDraft(), draft());
  write = async (value) => { stored = value; };
  await submitCreationDraft(catalog(), draft());
  assert.equal(nativeCalls().length, 1);
  assert.equal(hex(nativeCalls()[0][1].nonce), draft().nonce);
  assert.equal(stored, null);
});

test("lost native result keeps the original request and immutable native intent", async () => {
  const lost = new Error("result observer lost");
  await assert.rejects(submitCreationDraft(catalog(async () => { throw lost; }), draft()),
    (error) => error === lost);
  assert.deepEqual(await loadCreationDraft(), draft());
  await submitCreationDraft(catalog(), draft());
  const [first, second] = nativeCalls();
  assert.deepEqual(first.slice(1), second.slice(1));
  assert.equal(stored, null);
});

test("a different ID or changed input cannot replace a pending request", async () => {
  stored = JSON.stringify(draft());
  for (const replacement of [draft("cd"), draft("ab", "Changed label")]) {
    await assert.rejects(submitCreationDraft(catalog(), replacement), (error) => {
      assert.ok(error instanceof CreationDraftConflict);
      assert.deepEqual(error.pending, draft());
      return true;
    });
    assert.deepEqual(await loadCreationDraft(), draft());
  }
  assert.equal(nativeCalls().length, 0);
});

test("late native completion cannot erase a newer draft and native waits are not serialized", async () => {
  const firstEntered = Promise.withResolvers();
  const firstResult = Promise.withResolvers();
  const newEntered = Promise.withResolvers();
  const newResult = Promise.withResolvers();
  const old = submitCreationDraft(catalog(async () => { firstEntered.resolve(); return firstResult.promise; }), draft());
  const oldRejection = assert.rejects(old, (error) => {
    assert.ok(error instanceof CreationDraftConflict);
    assert.deepEqual(error.pending, draft("cd"));
    return true;
  });
  await firstEntered.promise;
  // Another mounted view recovers the same native result before the old waiter.
  await submitCreationDraft(catalog(), draft());
  const next = submitCreationDraft(catalog(async () => { newEntered.resolve(); return newResult.promise; }), draft("cd"));
  await newEntered.promise;
  firstResult.resolve();
  await oldRejection;
  assert.deepEqual(await loadCreationDraft(), draft("cd"));
  newResult.resolve();
  await next;
  assert.equal(stored, null);
});

test("new saves cannot interleave between an exact cleanup check and removal", async () => {
  const cleanupEntered = Promise.withResolvers();
  const allowCleanup = Promise.withResolvers();
  const newerEntered = Promise.withResolvers();
  const newerResult = Promise.withResolvers();
  let readCount = 0;
  read = async () => {
    const value = stored;
    if (++readCount === 2) { cleanupEntered.resolve(); await allowCleanup.promise; }
    return value;
  };
  const first = submitCreationDraft(catalog(), draft());
  await cleanupEntered.promise;
  const second = submitCreationDraft(catalog(async () => { newerEntered.resolve(); return newerResult.promise; }), draft("cd"));
  await nextTurn();
  assert.equal(readCount, 2, "new save bypassed the active cleanup section");
  assert.equal(nativeCalls().length, 1);
  allowCleanup.resolve();
  await first;
  await newerEntered.promise;
  assert.deepEqual(await loadCreationDraft(), draft("cd"));
  newerResult.resolve();
  await second;
  assert.equal(stored, null);
});

test("definite native metadata rejection unlocks only after matching cleanup", async () => {
  const invalid = new Authority(invalidMetadata);
  await assert.rejects(submitCreationDraft(catalog(async () => { throw invalid; }), draft()), (error) => {
    assert.ok(error instanceof CreationMetadataRejected);
    assert.deepEqual(error.draft, draft());
    assert.equal(error.cause, invalid);
    return true;
  });
  assert.equal(stored, null);
  await submitCreationDraft(catalog(), draft("cd", "Corrected"));
  assert.equal(stored, null);
});

test("uncertain cleanup never presents metadata correction as safely available", async () => {
  const uncertain = new Error("remove may have succeeded");
  remove = async () => { stored = null; throw uncertain; };
  await assert.rejects(submitCreationDraft(catalog(async () => { throw new Authority(invalidMetadata); }), draft()),
    (error) => error === uncertain && !(error instanceof CreationMetadataRejected));
  remove = async () => { stored = null; };
  await assert.rejects(submitCreationDraft(catalog(async () => { throw new Authority(invalidMetadata); }), draft()),
    CreationMetadataRejected);
  assert.equal(stored, null);
});

test("storage and unrelated authority errors preserve the request for exact recovery", async () => {
  for (const failure of [new Error("storage outcome unknown"), new Authority("StaleCommand"), null]) {
    await assert.rejects(submitCreationDraft(catalog(async () => { throw failure; }), draft()),
      (error) => error === failure);
    assert.deepEqual(await loadCreationDraft(), draft());
  }
  assert.equal(calls.filter(([kind]) => kind === "remove").length, 0);
});

test("caller mutation after submission cannot change native intent or exact cleanup", async () => {
  const input = { ...draft() };
  const submitted = submitCreationDraft(catalog(), input);
  input.localLabel = "Mutated";
  input.nonce = "cd".repeat(32);
  await submitted;
  assert.equal(nativeCalls()[0][2].metadata.localLabel, draft().localLabel);
  assert.equal(hex(nativeCalls()[0][1].nonce), draft().nonce);
  assert.equal(stored, null);
});


test("earlier nonempty-service draft is preserved and never replayed as an empty-service request", async () => {
  stored = JSON.stringify({ ...draft(), version: 1 });
  const original = stored;
  await assert.rejects(loadCreationDraft(), /kept for recovery/);
  await assert.rejects(submitCreationDraft(catalog(), draft()), /kept for recovery/);
  assert.equal(stored, original);
  assert.equal(nativeCalls().length, 0);
});

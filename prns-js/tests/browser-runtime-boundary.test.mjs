import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import { interfaceId } from "../dist/contract.js";
import { interfaceKey } from "../dist/browser/bytes.js";
import { RuntimeHost } from "../dist/browser/runtime.js";
import {
  packetFrame,
  packetFrameView,
} from "../dist/browser/values.js";

test("public packet frames copy while internal packet views retain owned storage", () => {
  const bytes = Uint8Array.of(0x21, 0x22);
  const copied = packetFrame(bytes);
  const adopted = packetFrameView(bytes);

  assert.notStrictEqual(copied, bytes);
  assert.strictEqual(adopted, bytes);
  bytes[0] = 0x31;
  assert.deepEqual(copied, Uint8Array.of(0x21, 0x22));
  assert.deepEqual(adopted, Uint8Array.of(0x31, 0x22));
});

test("interface keys preserve every identifier byte without textual encoding", () => {
  const first = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));
  const same = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));
  const different = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 8));

  assert.equal(interfaceKey(first), interfaceKey(same));
  assert.notEqual(interfaceKey(first), interfaceKey(different));
});

test("runtime ingress prefers the positional WASM binding", () => {
  const calls = [];
  const runtime = {
    ingest() {
      throw new Error("legacy ingress should not run");
    },
    ingestDirect(...values) {
      calls.push(values);
    },
  };
  const host = runtimeHost(runtime);
  const id = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));
  const packet = packetFrame(Uint8Array.of(0x21, 0x22));

  assert.deepEqual(host.ingest(id, packet), Tag("Accepted"));
  assert.deepEqual(calls, [[
    id,
    packet,
    42,
    Uint8Array.from({ length: 128 }, () => 0x31),
  ]]);
});

test("runtime ingress retains the object binding as a compatibility fallback", () => {
  const calls = [];
  const runtime = {
    ingest(options) {
      calls.push(options);
    },
  };
  const host = runtimeHost(runtime);
  const id = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));
  const packet = packetFrame(Uint8Array.of(0x41));

  assert.deepEqual(host.ingest(id, packet), Tag("Accepted"));
  assert.deepEqual(calls, [{
    interfaceId: id,
    bytes: packet,
    nowMs: 42,
    entropy: Uint8Array.from({ length: 128 }, () => 0x31),
  }]);
});

test("protocol crypto worker unavailability resumes the retained WASM operation inline", async () => {
  const calls = [];
  let taken = false;
  const runtime = protocolRuntime({
    takeBrowserWork() {
      if (taken) {
        return undefined;
      }
      taken = true;
      return {
        tag: "AnnounceVerify",
        data: {
          id: 7,
          publicKey: new Uint8Array(32),
          message: Uint8Array.of(1, 2, 3),
          signature: new Uint8Array(64),
        },
      };
    },
    completeBrowserWork(options) {
      calls.push(options);
    },
  });
  const executor = {
    verifyEd25519: async () => Tag("Unavailable"),
  };
  const host = runtimeHost(runtime, executor);
  const id = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));

  assert.deepEqual(host.ingest(id, packetFrame(Uint8Array.of(0x21))), Tag("Accepted"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, [{
    id: 7,
    outcome: "Unavailable",
    nowMs: 42,
    entropy: Uint8Array.from({ length: 128 }, () => 0x31),
  }]);
});

test("protocol link proof settlement lands the worker-derived shared secret", async () => {
  const calls = [];
  let taken = false;
  const runtime = protocolRuntime({
    takeBrowserWork() {
      if (taken) {
        return undefined;
      }
      taken = true;
      return {
        tag: "LinkProofVerify",
        data: {
          id: 9,
          publicKey: new Uint8Array(32),
          message: Uint8Array.of(1, 2, 3),
          signature: new Uint8Array(64),
          secretScalar: new Uint8Array(32),
          peerPublicKey: new Uint8Array(32),
        },
      };
    },
    completeBrowserWork(options) {
      calls.push(options);
    },
  });
  const sharedSecret = new Uint8Array(32).fill(0x44);
  const executor = {
    verifyLinkProof: async () => Tag("Verified", { sharedSecret }),
  };
  const host = runtimeHost(runtime, executor);
  const id = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));

  assert.deepEqual(host.ingest(id, packetFrame(Uint8Array.of(0x21))), Tag("Accepted"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, [{
    id: 9,
    outcome: "Verified",
    sharedSecret,
    nowMs: 42,
    entropy: Uint8Array.from({ length: 128 }, () => 0x31),
  }]);
});

test("resource seal work returns its worker bytes through the unified completion", async () => {
  const calls = [];
  let taken = false;
  const sealed = Uint8Array.of(7, 8, 9);
  const runtime = protocolRuntime({
    takeBrowserWork() {
      if (taken) {
        return undefined;
      }
      taken = true;
      return {
        tag: "ResourceSeal",
        data: {
          id: 11,
          linkId: new Uint8Array(16),
          noncePrefixedBytes: 4,
          totalSegments: 1,
          plaintext: Uint8Array.of(1, 2, 3, 4),
          signingKey: new Uint8Array(32),
          encryptionKey: new Uint8Array(32),
          sealIv: new Uint8Array(16),
          salts: new Uint8Array(32),
        },
      };
    },
    completeBrowserWork(options) {
      calls.push(options);
      return { tag: "Applied" };
    },
  });
  const executor = {
    seal: async ({ plaintext }) => Tag("Sealed", { sealed, plaintext }),
  };
  const host = runtimeHost(runtime, executor);
  const id = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));

  assert.deepEqual(host.ingest(id, packetFrame(Uint8Array.of(0x21))), Tag("Accepted"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, [{
    id: 11,
    outcome: "Sealed",
    sealed,
    nowMs: 42,
    entropy: Uint8Array.from({ length: 128 }, () => 0x31),
  }]);
});

test("an invalid resource open landing retries through the retained inline work", async () => {
  const calls = [];
  let taken = false;
  const plaintext = Uint8Array.of(4, 3, 2, 1);
  const runtime = protocolRuntime({
    takeBrowserWork() {
      if (taken) {
        return undefined;
      }
      taken = true;
      return {
        tag: "WholeResourceOpen",
        data: {
          id: 13,
          linkId: new Uint8Array(16),
          hash: new Uint8Array(32),
          signingKey: new Uint8Array(32),
          encryptionKey: new Uint8Array(32),
          sealed: Uint8Array.of(9),
          hashPlan: { tag: "AfterDecompression", data: undefined },
          totalSegments: 1,
        },
      };
    },
    completeBrowserWork(options) {
      calls.push(options);
      return { tag: calls.length === 1 ? "Invalid" : "Applied" };
    },
  });
  const executor = {
    open: async () => Tag("Opened", plaintext),
  };
  const host = runtimeHost(runtime, executor);
  const id = interfaceId(Uint8Array.of(23, 1, 2, 3, 4, 5, 6, 7));

  assert.deepEqual(host.ingest(id, packetFrame(Uint8Array.of(0x21))), Tag("Accepted"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, [
    {
      id: 13,
      outcome: "Opened",
      plaintext,
      nowMs: 42,
      entropy: Uint8Array.from({ length: 128 }, () => 0x31),
    },
    {
      id: 13,
      outcome: "Unavailable",
      nowMs: 42,
      entropy: Uint8Array.from({ length: 128 }, () => 0x31),
    },
  ]);
});

function protocolRuntime(overrides) {
  return {
    ingestDirect() {},
    configureBrowserWork() {},
    takeBrowserWork() {},
    completeBrowserWork() {},
    ...overrides,
  };
}

function runtimeHost(runtime, cryptoExecutor) {
  return new RuntimeHost(
    {},
    {
      configureBrowserWork() {},
      takeBrowserWork() {},
      completeBrowserWork() {},
      ...runtime,
    },
    (length) => Tag("Filled", Uint8Array.from({ length }, () => 0x31)),
    () => 42,
    Tag("Available", new Uint8Array(16)),
    cryptoExecutor,
    () => undefined,
  );
}

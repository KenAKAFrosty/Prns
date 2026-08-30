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

function runtimeHost(runtime) {
  return new RuntimeHost(
    {},
    runtime,
    (length) => Tag("Filled", Uint8Array.from({ length }, () => 0x31)),
    () => 42,
    Tag("Available", new Uint8Array(16)),
    Tag("PortableWasm"),
    () => undefined,
  );
}

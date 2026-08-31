import assert from "node:assert/strict";
import test from "node:test";

import {
  prepareByteTransfer,
  receiveByteTransfer,
} from "../dist/browser/byte_transfer.js";
import { interfaceId } from "../dist/contract.js";
import {
  prepareIngressTransfer,
  receiveIngressTransfer,
} from "../dist/browser/worker_network_protocol.js";

test("donates each independently owned backing store once", () => {
  const buffer = new ArrayBuffer(16);
  const bytes = new Uint8Array(buffer);
  bytes.set([1, 2, 3, 4], 4);
  bytes.set([5, 6, 7], 10);

  const batch = prepareByteTransfer([
    new Uint8Array(buffer, 4, 4),
    new Uint8Array(buffer, 10, 3),
  ]);

  assert.deepEqual(batch.buffers, [buffer]);
  assert.deepEqual(
    receiveByteTransfer(batch).map((value) => [...value]),
    [[1, 2, 3, 4], [5, 6, 7]],
  );

  const received = structuredClone(batch, { transfer: [...batch.buffers] });
  assert.equal(buffer.byteLength, 0);
  assert.deepEqual(
    receiveByteTransfer(received).map((value) => [...value]),
    [[1, 2, 3, 4], [5, 6, 7]],
  );
});

test("copies retained backing stores into one transferable buffer", () => {
  const retained = Uint8Array.from([9, 8, 7, 6, 5, 4]);
  const batch = prepareByteTransfer(
    [retained.subarray(1, 3), retained.subarray(4, 6)],
    new Set([retained.buffer]),
  );

  assert.equal(batch.buffers.length, 1);
  assert.notEqual(batch.buffers[0], retained.buffer);
  assert.deepEqual(
    receiveByteTransfer(batch).map((value) => [...value]),
    [[8, 7], [5, 4]],
  );
});

test("rejects malformed spans before constructing a view", () => {
  assert.throws(
    () => receiveByteTransfer({
      buffers: [new ArrayBuffer(4)],
      spans: [{ bufferIndex: 0, byteOffset: 3, byteLength: 2 }],
    }),
    /exceeds its buffer/,
  );
});

test("transfers ingress as tiny interface facts and donated packet buffers", () => {
  const first = interfaceId(Uint8Array.of(1, 0, 0, 0, 0, 0, 0, 0));
  const second = interfaceId(Uint8Array.of(2, 0, 0, 0, 0, 0, 0, 0));
  const payload = Uint8Array.from([0x21, 0x22, 0x31]);
  const batch = prepareIngressTransfer([
    { interfaceId: first, bytes: payload.subarray(0, 2) },
    { interfaceId: second, bytes: payload.subarray(2, 3) },
  ]);

  const received = structuredClone(batch, {
    transfer: [...batch.bytes.buffers],
  });

  assert.equal(payload.buffer.byteLength, 0);
  assert.deepEqual(
    receiveIngressTransfer(received).map((item) => ({
      interfaceId: [...item.interfaceId],
      bytes: [...item.bytes],
    })),
    [
      { interfaceId: [...first], bytes: [0x21, 0x22] },
      { interfaceId: [...second], bytes: [0x31] },
    ],
  );
});

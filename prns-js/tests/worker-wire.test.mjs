import assert from "node:assert/strict";
import { test } from "node:test";

import {
  decodePackedValue,
  inferPackedValue,
  packingSupport,
} from "../dist/worker_wire/inferred_codec.js";
import {
  MAXIMUM_WIRE_BATCH_ITEMS,
  WireBatchDecoder,
  WireBatchEncoder,
} from "../dist/worker_wire/wire_batch.js";
import {
  BatchedPortReceiver,
  BatchedPortSender,
} from "../dist/worker_wire/batched_port.js";
import { Tag } from "../dist/casework.js";

function packed(value) {
  const outcome = inferPackedValue(value);
  assert.equal(outcome.tag, "Packed");
  return outcome.data;
}

test("runtime inference round trips practical nested TypeScript values", () => {
  const values = Array.from({ length: 257 }, (_, index) => ({
    stuff: index % 3 === 0 ? "alpha" : "beta",
    name: `node-${index}`,
    uuid: `00000000-0000-0000-0000-${String(index).padStart(12, "0")}`,
    nested: [
      { score: index + 0.25, recent: index % 2 === 0 },
      { score: index + 0.75, recent: index % 5 === 0 },
    ],
    last_seen_at: new Date(1_700_000_000_000 + index),
    enabled: index % 7 === 0,
    optional: index % 4 === 0 ? undefined : index,
    nullable: index % 6 === 0 ? null : index * 2,
    state: index % 2 === 0
      ? Tag("Ready", { peers: index })
      : Tag("Waiting", { since: new Date(1_600_000_000_000 + index) }),
    bytes: Uint8Array.of(index & 0xff, (index + 1) & 0xff),
  }));
  delete values[3].optional;
  const encoded = packed(values);
  const decoded = decodePackedValue(encoded.schema, encoded.buffer);
  assert.deepEqual(decoded, values);
  assert.equal(Object.hasOwn(decoded[3], "optional"), false);
  assert.equal(encoded.buffer.byteLength < JSON.stringify(values).length, true);
});

test("numeric and empty values preserve their exact observable shape", () => {
  const value = [NaN, Infinity, -Infinity, -0, 0, [], "", null, undefined];
  const encoded = packed(value);
  const decoded = decodePackedValue(encoded.schema, encoded.buffer);
  assert.equal(Number.isNaN(decoded[0]), true);
  assert.equal(decoded[1], Infinity);
  assert.equal(decoded[2], -Infinity);
  assert.equal(Object.is(decoded[3], -0), true);
  assert.deepEqual(decoded.slice(4), value.slice(4));
});

test("record decoding preserves data properties named __proto__", () => {
  const value = JSON.parse('{"__proto__":{"safe":true},"name":"node"}');
  const encoded = packed(value);
  const decoded = decodePackedValue(encoded.schema, encoded.buffer);
  assert.deepEqual(decoded, value);
  assert.equal(Object.hasOwn(decoded, "__proto__"), true);
  assert.equal(Object.getPrototypeOf(decoded), Object.prototype);
});

test("repeated booleans occupy bitplanes", () => {
  const value = Array.from({ length: 4_096 }, (_, index) => index % 3 === 0);
  const encoded = packed(value);
  assert.equal(encoded.buffer.byteLength < 600, true);
  assert.deepEqual(decodePackedValue(encoded.schema, encoded.buffer), value);
});

test("caller-owned byte buffers remain attached", () => {
  const bytes = new Uint8Array(128 * 1024);
  bytes[bytes.length - 1] = 91;
  const encoded = packed({ bytes });
  assert.equal(bytes.byteLength, 128 * 1024);
  assert.equal(bytes[bytes.length - 1], 91);
  assert.deepEqual(decodePackedValue(encoded.schema, encoded.buffer), { bytes });
});

test("identity-sensitive and unsupported values decline to clone", () => {
  class CustomValue {
    value = 1;
  }
  const shared = { value: 1 };
  const cycle = {};
  cycle.self = cycle;
  const sparse = new Array(3);
  sparse[2] = 1;
  let accessorReads = 0;
  const accessor = [1];
  Object.defineProperty(accessor, "0", {
    enumerable: true,
    get: () => {
      accessorReads += 1;
      return 1;
    },
  });
  const symbol = [1];
  Object.defineProperty(symbol, Symbol("value"), {
    enumerable: true,
    value: 2,
  });
  assert.equal(packingSupport(new CustomValue()).tag, "Declined");
  assert.equal(packingSupport([shared, shared]).tag, "Declined");
  assert.equal(packingSupport(cycle).tag, "Declined");
  assert.equal(packingSupport(sparse).tag, "Declined");
  assert.equal(packingSupport(accessor).tag, "Declined");
  assert.equal(accessorReads, 0);
  assert.equal(packingSupport(symbol).tag, "Declined");
  assert.equal(packingSupport(new Blob(["hello"])).tag, "Declined");
});

test("wire batches preserve order across packed and clone planes", () => {
  const encoder = new WireBatchEncoder({ minimumItems: 1 });
  const decoder = new WireBatchDecoder();
  const values = Array.from({ length: 20 }, (_, index) =>
    index === 7 ? new Blob(["clone-plane"]) : Tag("Value", { index, active: index % 2 === 0 })
  );
  const first = encoder.encode(values);
  assert.equal(first.message.tag, "PackedBatch");
  assert.deepEqual(decoder.decode(first.message), values);
  const second = encoder.encode(values.filter((_, index) => index !== 7));
  assert.equal(second.message.tag, "PackedBatch");
  assert.equal(second.message.data.schema, undefined);
  assert.deepEqual(decoder.decode(second.message), values.filter((_, index) => index !== 7));
});

test("warm schemas re-infer changed shapes instead of dropping fields", () => {
  const encoder = new WireBatchEncoder({ minimumItems: 1 });
  const decoder = new WireBatchDecoder();
  const first = encoder.encode([{ value: 1 }]);
  assert.deepEqual(decoder.decode(first.message), [{ value: 1 }]);
  const changed = encoder.encode([{ value: 2, added: "preserved" }]);
  assert.equal(changed.message.tag, "PackedBatch");
  assert.notEqual(changed.message.data.schema, undefined);
  assert.deepEqual(decoder.decode(changed.message), [{ value: 2, added: "preserved" }]);
});

test("warm schemas decline shared identity to structured clone", () => {
  const encoder = new WireBatchEncoder({ minimumItems: 1 });
  encoder.encode([{ value: 1 }, { value: 2 }]);
  const shared = { value: 3 };
  const encoded = encoder.encode([shared, shared]);
  assert.equal(encoded.message.tag, "ClonedBatch");
  assert.equal(encoded.message.data.values[0], encoded.message.data.values[1]);
});

test("the measured default keeps ordinary batches on native clone", () => {
  const encoded = new WireBatchEncoder().encode(
    Array.from(
      { length: MAXIMUM_WIRE_BATCH_ITEMS },
      (_, index) => ({ index, active: index % 2 === 0 }),
    ),
  );
  assert.equal(encoded.message.tag, "ClonedBatch");
});

test("the encoder rejects batches outside the structural item bound", () => {
  assert.throws(
    () => new WireBatchEncoder().encode(
      new Array(MAXIMUM_WIRE_BATCH_ITEMS + 1).fill(undefined),
    ),
    /item bound/,
  );
});

test("explicit typed codecs own their packed crossing", () => {
  const codec = {
    id: "u32-values-v1",
    encode: (values) => Uint32Array.from(values).buffer,
    decode: (buffer) => [...new Uint32Array(buffer)],
  };
  const encoder = new WireBatchEncoder({ codec });
  const decoder = new WireBatchDecoder([codec]);
  const encoded = encoder.encode([1, 2, 3, 0xffff_ffff]);
  assert.equal(encoded.message.tag, "CodecBatch");
  assert.deepEqual(encoded.transfer, [encoded.message.data.payload]);
  assert.deepEqual(decoder.decode(encoded.message), [1, 2, 3, 0xffff_ffff]);
  assert.throws(
    () => new WireBatchDecoder().decode(encoded.message),
    /unknown codec/,
  );
});

test("unknown schema references fail closed", () => {
  const encoded = new WireBatchEncoder({ minimumItems: 1 }).encode(
    Array.from({ length: 10 }, (_, index) => ({ index })),
  );
  assert.equal(encoded.message.tag, "PackedBatch");
  const missing = Tag("PackedBatch", {
    ...encoded.message.data,
    schema: undefined,
    fingerprint: undefined,
  });
  assert.throws(
    () => new WireBatchDecoder().decode(missing),
    /unknown schema/,
  );
});

test("same-turn sends coalesce and later grains yield through tasks", async () => {
  const messages = [];
  const tasks = [];
  const sender = new BatchedPortSender({
    port: {
      postMessage: (message) => messages.push(message),
    },
    wrap: (batch) => Tag("Frame", { batch }),
    maximumItems: 4,
    maximumBytes: 1024 * 1024,
    scheduleTask: (task) => tasks.push(task),
    failed: assert.fail,
  });
  for (let index = 0; index < 10; index += 1) {
    sender.send({ index, value: index * 2 });
  }
  assert.equal(messages.length, 0);
  await Promise.resolve();
  assert.equal(messages.length, 1);
  assert.equal(tasks.length, 1);
  while (tasks.length > 0) {
    tasks.shift()();
  }
  assert.equal(messages.length, 3);
  const received = [];
  const receiver = new BatchedPortReceiver((value) => received.push(value));
  for (const message of messages) {
    receiver.receive(message.data.batch);
  }
  assert.deepEqual(
    received,
    Array.from({ length: 10 }, (_, index) => ({ index, value: index * 2 })),
  );
});

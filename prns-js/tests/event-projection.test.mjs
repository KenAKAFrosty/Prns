import assert from "node:assert/strict";
import test from "node:test";

import {
  EventBatchProjectionError,
  decodeEventBatchProjection,
  retainApplicationEventBatchProjection,
  summarizeEventBatchProjection,
} from "../dist/event_projection.js";

const linkEstablishedVector = Uint8Array.from([
  80, 82, 78, 69, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 26, 0, 0, 0, 201,
  0, 2, 0, 4, 0, 1, 0, 2, 0, 0, 0, 1, 2, 8, 0, 3, 0, 8, 0, 0, 0, 9, 0,
  0, 0, 0, 0, 0, 0,
]);

const commandCorrelationVector = Uint8Array.from([
  80, 82, 78, 69, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 102,
  0, 1, 0, 0, 128, 3, 0, 8, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0,
]);

const singleDeliveryPayloadVector = Uint8Array.from([
  80, 82, 78, 69, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 12, 0, 0, 0, 100,
  0, 1, 0, 3, 0, 1, 0, 4, 0, 0, 0, 1, 2, 3, 4,
]);

const mixedVector = new Uint8Array(
  singleDeliveryPayloadVector.byteLength + linkEstablishedVector.byteLength - 16,
);
mixedVector.set(singleDeliveryPayloadVector);
new DataView(mixedVector.buffer).setUint32(12, 2, true);
mixedVector.set(linkEstablishedVector.subarray(16), singleDeliveryPayloadVector.byteLength);

test("decodes the shared Rust event projection vector", () => {
  const decoded = decodeEventBatchProjection(linkEstablishedVector);
  assert.deepEqual(decoded, [
    {
      kind: 201,
      fields: new Map([
        [4, linkEstablishedVector.subarray(32, 34)],
        [8, 9n],
      ]),
    },
  ]);
});

test("rejects truncated event batches with a typed error", () => {
  assert.throws(
    () => decodeEventBatchProjection(linkEstablishedVector.subarray(0, 47)),
    (error) =>
      error instanceof EventBatchProjectionError && error.code === "Truncated",
  );
});

test("decodes the shared command-correlation extension vector", () => {
  assert.deepEqual(decodeEventBatchProjection(commandCorrelationVector), [
    {
      kind: 102,
      fields: new Map([[32_768, 7n]]),
    },
  ]);
});

test("summarizes projected event ownership without materializing public events", () => {
  assert.deepEqual(summarizeEventBatchProjection(linkEstablishedVector), {
    applicationEvents: 0,
    diagnostics: 1,
    retainedEventBytes: 0,
  });
  assert.deepEqual(summarizeEventBatchProjection(commandCorrelationVector), {
    applicationEvents: 1,
    diagnostics: 0,
    retainedEventBytes: 0,
  });
  assert.deepEqual(summarizeEventBatchProjection(mixedVector), {
    applicationEvents: 1,
    diagnostics: 1,
    retainedEventBytes: 4,
  });
});

test("retains application records when diagnostic transport pressure drops a batch", () => {
  assert.deepEqual(
    retainApplicationEventBatchProjection(mixedVector),
    singleDeliveryPayloadVector,
  );
});

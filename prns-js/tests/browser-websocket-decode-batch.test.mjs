import assert from "node:assert/strict";
import test from "node:test";

import { parseWebSocketDecodeBatch } from "../dist/browser/websocket/decode_batch.js";

test("materializes packed WebSocket packets and a resolved write as byte views", () => {
  const writer = decodeBatchWriter(2, 1);
  writer.bytes(Uint8Array.of(0x21, 0x22));
  writer.bytes(Uint8Array.of(0x31));
  writer.bytes(Uint8Array.of(0x41, 0x42, 0x43));

  assert.deepEqual(parseWebSocketDecodeBatch(writer.finish()), {
    packets: [Uint8Array.of(0x21, 0x22), Uint8Array.of(0x31)],
    resolvedOutbound: Uint8Array.of(0x41, 0x42, 0x43),
  });
});

test("rejects incompatible and non-canonical WebSocket decode batches", () => {
  const badMagic = decodeBatchWriter(0, 0).finish();
  badMagic[0] = 0;
  assert.throws(
    () => parseWebSocketDecodeBatch(badMagic),
    /magic is invalid/,
  );

  const badVersion = decodeBatchWriter(0, 0).finish();
  badVersion[4] = 2;
  assert.throws(
    () => parseWebSocketDecodeBatch(badVersion),
    /version is unsupported/,
  );

  const badFlags = decodeBatchWriter(0, 2).finish();
  assert.throws(
    () => parseWebSocketDecodeBatch(badFlags),
    /flags are unsupported/,
  );

  const impossibleCount = decodeBatchWriter(1, 0).finish();
  assert.throws(
    () => parseWebSocketDecodeBatch(impossibleCount),
    /packet count exceeds its bytes/,
  );

  const emptyPacket = decodeBatchWriter(1, 0);
  emptyPacket.bytes(new Uint8Array());
  assert.throws(
    () => parseWebSocketDecodeBatch(emptyPacket.finish()),
    /contains an empty packet/,
  );

  const trailing = decodeBatchWriter(0, 0).finish();
  const withTrailingByte = new Uint8Array(trailing.byteLength + 1);
  withTrailingByte.set(trailing);
  assert.throws(
    () => parseWebSocketDecodeBatch(withTrailingByte),
    /trailing bytes/,
  );
});

function decodeBatchWriter(count, flags) {
  const values = [];
  const writer = {
    u16(value) {
      values.push(value & 0xff, value >>> 8 & 0xff);
    },
    u32(value) {
      values.push(
        value & 0xff,
        value >>> 8 & 0xff,
        value >>> 16 & 0xff,
        value >>> 24 & 0xff,
      );
    },
    bytes(bytes) {
      writer.u32(bytes.byteLength);
      values.push(...bytes);
    },
    finish() {
      return Uint8Array.from(values);
    },
  };
  writer.u32(0x4453_5750);
  writer.u16(1);
  writer.u16(flags);
  writer.u32(count);
  return writer;
}

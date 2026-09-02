import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import { interfaceId } from "../dist/contract.js";
import { parseOutboundBatch } from "../dist/browser/outbound_batch.js";

test("materializes every packed WASM outbound shape without string decoding", () => {
  const direct = interfaceId(Uint8Array.of(4, 1, 2, 3, 4, 5, 6, 7));
  const selected = interfaceId(Uint8Array.of(13, 8, 9, 10, 11, 12, 13, 14));
  const writer = outboundBatchWriter(4);
  writer.u8(0);
  writer.u8(0);
  writer.fixed(direct);
  writer.bytes(Uint8Array.of(0x21, 0x22));
  writer.u8(1);
  writer.u8(3);
  writer.u8(1);
  writer.u8(7);
  writer.u8(0);
  writer.bytes(Uint8Array.of(0x31));
  writer.u8(0);
  writer.u8(1);
  writer.u8(12);
  writer.u8(1);
  writer.fixed(selected);
  writer.bytes(Uint8Array.of(0x41, 0x42, 0x43));
  writer.u8(0);
  writer.u8(1);
  writer.u8(5);
  writer.u8(0);
  writer.bytes(Uint8Array.of(0x51));

  assert.deepEqual(parseOutboundBatch(writer.finish()), [
    {
      type: "frame",
      target: Tag("Interface", direct),
      bytes: Uint8Array.of(0x21, 0x22),
    },
    {
      type: "announce",
      target: Tag("Broadcast", {
        supervisorKind: "auto-wifi",
        fan: Tag("All"),
      }),
      hops: 3,
      bytes: Uint8Array.of(0x31),
    },
    {
      type: "frame",
      target: Tag("Broadcast", {
        supervisorKind: "bluetooth-auto",
        fan: Tag("Only", selected),
      }),
      bytes: Uint8Array.of(0x41, 0x42, 0x43),
    },
    {
      type: "frame",
      target: Tag("Broadcast", {
        supervisorKind: "auto-usb-host",
        fan: Tag("All"),
      }),
      bytes: Uint8Array.of(0x51),
    },
  ]);
});

test("rejects incompatible and non-canonical outbound batches", () => {
  const badMagic = outboundBatchWriter(0).finish();
  badMagic[0] = 0;
  assert.throws(() => parseOutboundBatch(badMagic), /magic is invalid/);

  const badVersion = outboundBatchWriter(0, 2).finish();
  assert.throws(() => parseOutboundBatch(badVersion), /version is unsupported/);

  const reserved = outboundBatchWriter(0).finish();
  reserved[6] = 1;
  assert.throws(() => parseOutboundBatch(reserved), /reserved bits are set/);

  const trailing = outboundBatchWriter(0).finish();
  const withTrailingByte = new Uint8Array(trailing.byteLength + 1);
  withTrailingByte.set(trailing);
  assert.throws(() => parseOutboundBatch(withTrailingByte), /trailing bytes/);

  const impossibleCount = outboundBatchWriter(1).finish();
  assert.throws(
    () => parseOutboundBatch(impossibleCount),
    /record count exceeds its bytes/,
  );

  const unknownSupervisor = outboundBatchWriter(1);
  unknownSupervisor.u8(0);
  unknownSupervisor.u8(1);
  unknownSupervisor.u8(255);
  unknownSupervisor.u8(0);
  unknownSupervisor.bytes(Uint8Array.of(1));
  assert.throws(
    () => parseOutboundBatch(unknownSupervisor.finish()),
    /unknown interface kind code 255/,
  );

  const unknownFan = outboundBatchWriter(1);
  unknownFan.u8(0);
  unknownFan.u8(1);
  unknownFan.u8(7);
  unknownFan.u8(255);
  unknownFan.fixed(Uint8Array.of(0, 0, 0, 0));
  assert.throws(
    () => parseOutboundBatch(unknownFan.finish()),
    /fan target is unknown/,
  );
});

function outboundBatchWriter(count, version = 1) {
  const values = [];
  const writer = {
    u8(value) {
      values.push(value);
    },
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
    fixed(bytes) {
      values.push(...bytes);
    },
    bytes(bytes) {
      writer.u32(bytes.byteLength);
      writer.fixed(bytes);
    },
    finish() {
      return Uint8Array.from(values);
    },
  };
  writer.u32(0x5455_4f50);
  writer.u16(version);
  writer.u16(0);
  writer.u32(count);
  return writer;
}

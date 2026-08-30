import assert from "node:assert/strict";
import test from "node:test";

import { Tag } from "../dist/casework.js";
import { packetHash } from "../dist/contract.js";
import {
  COMMAND_FAILURE_KIND_CODES,
  COMMAND_OUTCOME_KIND_CODES,
  DELIVERY_EVIDENCE_KIND_CODES,
  HOST_SCHEMA_VERSION,
  PACKET_HASH_LENGTH,
} from "../dist/contract.generated.js";
import { parseCommandSettlementBatch } from "../dist/browser/command_settlement_batch.js";

const textEncoder = new TextEncoder();

test("materializes packed WASM command settlements at the consumer edge", () => {
  const hash = Uint8Array.from({ length: PACKET_HASH_LENGTH }, (_, index) => index);
  const writer = settlementBatchWriter(4);
  writer.u64(7n);
  writer.u8(0);
  writer.u32(COMMAND_OUTCOME_KIND_CODES.Announced);
  writer.u64(8n);
  writer.u8(0);
  writer.u32(COMMAND_OUTCOME_KIND_CODES.PacketDelivered);
  writer.u64(42n);
  writer.u32(DELIVERY_EVIDENCE_KIND_CODES.ExplicitProof);
  writer.u8(1);
  writer.fixed(hash);
  writer.u64(9n);
  writer.u8(1);
  writer.u32(COMMAND_FAILURE_KIND_CODES.WriteFailed);
  writer.string("radio unavailable");
  writer.u64(10n);
  writer.u8(2);

  assert.deepEqual(parseCommandSettlementBatch(writer.finish()), [
    Tag("CommandSettled", {
      commandId: 7n,
      settlement: Tag("Succeeded", Tag("Announced")),
    }),
    Tag("CommandSettled", {
      commandId: 8n,
      settlement: Tag(
        "Succeeded",
        Tag("PacketDelivered", {
          rttMillis: 42,
          evidence: "ExplicitProof",
          packetHash: packetHash(hash),
        }),
      ),
    }),
    Tag("CommandSettled", {
      commandId: 9n,
      settlement: Tag(
        "Failed",
        Tag("WriteFailed", { detail: "radio unavailable" }),
      ),
    }),
    Tag("CommandSettled", { commandId: 10n }),
  ]);
});

test("rejects incompatible and non-canonical settlement batches", () => {
  const schemaMismatch = settlementBatchWriter(0, HOST_SCHEMA_VERSION + 1).finish();
  assert.throws(
    () => parseCommandSettlementBatch(schemaMismatch),
    /schema is incompatible/,
  );

  const trailing = settlementBatchWriter(0).finish();
  const withTrailingByte = new Uint8Array(trailing.byteLength + 1);
  withTrailingByte.set(trailing);
  assert.throws(
    () => parseCommandSettlementBatch(withTrailingByte),
    /trailing bytes/,
  );

  const impossibleCount = settlementBatchWriter(1).finish();
  assert.throws(
    () => parseCommandSettlementBatch(impossibleCount),
    /record count exceeds its bytes/,
  );
});

function settlementBatchWriter(count, schemaVersion = HOST_SCHEMA_VERSION) {
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
    u64(value) {
      for (let offset = 0n; offset < 64n; offset += 8n) {
        values.push(Number(value >> offset & 0xffn));
      }
    },
    fixed(bytes) {
      values.push(...bytes);
    },
    string(value) {
      const bytes = textEncoder.encode(value);
      writer.u32(bytes.byteLength);
      writer.fixed(bytes);
    },
    finish() {
      return Uint8Array.from(values);
    },
  };
  writer.u32(0x4353_5250);
  writer.u16(1);
  writer.u16(0);
  writer.u32(schemaVersion);
  writer.u32(count);
  return writer;
}

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
import {
  MINIMUM_WORKER_CODEC_ITEMS,
  workerInvocationCodec,
  workerSettlementCodec,
} from "../dist/browser/worker_codecs.js";
import {
  DESTINATION_HASH_LENGTH,
  IDENTITY_HASH_LENGTH,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  PACKET_HASH_LENGTH,
  REQUEST_ID_LENGTH,
  REQUEST_PATH_HASH_LENGTH,
} from "../dist/contract.js";
import { Tag } from "../dist/casework.js";

function packed(value) {
  const outcome = inferPackedValue(value);
  assert.equal(outcome.tag, "Packed");
  return outcome.data;
}

function fixedBytes(length, seed) {
  return Uint8Array.from({ length }, (_, index) => (seed + index) & 0xff);
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

test("the Prns invocation codec round trips every hot command shape", () => {
  const destination = fixedBytes(DESTINATION_HASH_LENGTH, 1);
  const interfaceId = fixedBytes(INTERFACE_ID_LENGTH, 2);
  const linkId = fixedBytes(LINK_ID_LENGTH, 3);
  const identity = fixedBytes(IDENTITY_HASH_LENGTH, 4);
  const pathHash = fixedBytes(REQUEST_PATH_HASH_LENGTH, 5);
  const requestId = fixedBytes(REQUEST_ID_LENGTH, 6);
  const commands = [
    Tag("Announce", { destination }),
    Tag("Announce", { destination, interface: interfaceId }),
    Tag("SendSinglePacket", { destination, payload: Uint8Array.of(1, 2, 3) }),
    Tag("CloseLink", { linkId }),
    Tag("DetachInterface", { interface: interfaceId }),
    Tag("EstablishLink", { destination }),
    Tag("RequestPath", { destination }),
    Tag("Identify", { linkId, identity }),
    Tag("SendLinkPacket", { linkId, payload: Uint8Array.of(4, 5) }),
    Tag("Request", {
      linkId,
      pathHash,
      payload: Uint8Array.of(6, 7),
      timeout: Tag("LinkDefault"),
    }),
    Tag("Request", {
      linkId,
      pathHash,
      payload: Uint8Array.of(8, 9),
      timeout: Tag("Exact", { millis: 1250 }),
      maximumResponseBytes: 65_536,
    }),
    Tag("Respond", {
      linkId,
      requestId,
      requestRttMillis: 12.5,
      payload: Uint8Array.of(10, 11),
    }),
    Tag("SendResource", {
      linkId,
      payload: Uint8Array.of(12, 13),
      compression: Tag("Auto"),
    }),
    Tag("SendResource", {
      linkId,
      payload: Uint8Array.of(14, 15),
      packedMetadata: Uint8Array.of(16),
      compression: Tag("Never"),
    }),
    Tag("SetLinkResourceStrategy", {
      linkId,
      strategy: Tag("Accept", {
        maximumUncompressedBytes: 1_000_000,
        acceptCompressed: true,
      }),
    }),
    Tag("SetDestinationResourceStrategy", {
      destination,
      strategy: Tag("Refuse"),
    }),
    Tag("SendChannelMessage", {
      linkId,
      messageType: 17,
      payload: Uint8Array.of(18, 19),
    }),
    Tag("AllowRequester", { destination, pathHash, identity }),
  ];
  const values = commands.map((command, index) => ({
    id: index + 1,
    call: Tag("Execute", command),
  }));
  const encoder = new WireBatchEncoder({ codec: workerInvocationCodec });
  const decoder = new WireBatchDecoder([workerInvocationCodec]);
  const encoded = encoder.encode(values);
  assert.equal(encoded.message.tag, "CodecBatch");
  assert.deepEqual(decoder.decode(encoded.message), values);
  assert.equal(commands[2].data.payload.byteLength, 3);
});

test("the Prns settlement codec round trips every outcome and failure shape", () => {
  const interfaceId = fixedBytes(INTERFACE_ID_LENGTH, 20);
  const linkId = fixedBytes(LINK_ID_LENGTH, 21);
  const packetHash = fixedBytes(PACKET_HASH_LENGTH, 22);
  const outcomes = [
    Tag("Announced"),
    Tag("PacketDelivered", {
      rttMillis: 4.5,
      evidence: "ExplicitProof",
      packetHash,
    }),
    Tag("PacketDelivered", { rttMillis: 5.5, evidence: "ImplicitProof" }),
    Tag("PacketDelivered", { rttMillis: 6.5, evidence: "Response" }),
    Tag("LinkCloseQueued"),
    Tag("InterfaceAttached", { interface: interfaceId }),
    Tag("InterfaceDetached", { interface: interfaceId }),
    Tag("LinkEstablished", { linkId, rttMillis: 7.5 }),
    Tag("PathDiscovered", { hops: 3 }),
    Tag("Identified"),
    Tag("ResponseReceived", { data: Uint8Array.of(1, 2, 3), rttMillis: 8.5 }),
    Tag("ResponseSent", { rttMillis: 9.5 }),
    Tag("ResourceSent"),
    Tag("ResourceStrategySet"),
    Tag("RequesterAllowed"),
  ];
  const failureTags = [
    "NodeStopped", "Busy", "PayloadTooLarge", "UnknownDestination",
    "NotSingleDestination", "AnnounceAppDataTooLong", "UnknownInterface",
    "NoRouteToDestination", "NotDirectlyReachable", "PacketCulled",
    "DeliveryTimedOut", "InvalidBitrate", "BindFailed", "WriteFailed",
    "UnsupportedByBackend", "UnknownLink", "LinkNotActive", "EntropyUnavailable",
    "NotLinkInitiator", "IdentityNotHeld", "UnknownRequestHandler",
    "RequestPolicyNotAllowList", "RequestAllowListFull", "LinkBusy",
    "ResourceTableFull", "ResourceMetadataTooLarge", "ResourceRejectedByPeer",
    "ResourceSequencingFailed", "ResourcePredecessorFailed", "ChannelWindowFull",
    "ChannelUntrackable", "InvalidChannelMessageType", "InvalidConfiguration",
    "ResourceUploadCancelled", "ResourceEarlyEof", "ResourceLengthOverrun",
    "PermissionDenied", "DeviceUnavailable", "ConnectFailed", "BackendFailed",
    "ResponseTooLarge", "LinkClosed", "ResponseCancelledBySender",
    "ResponseHashmapBeyondPartCount", "ResponseHashmapSkipsAhead",
    "ResponseHashmapTooLong", "ResponseHashmapRagged", "ResponseRetriesExhausted",
    "ResponseLinkVanished", "ResponseTransferUnopenable", "ResponseTransferCorrupt",
    "ResponseProofUnsendable", "ResponseDecompressionFailed",
    "ResponseDecompressionTimedOut", "ResponseOpenTimedOut",
    "ResponseMetadataOverrun",
  ];
  const detailFailures = new Set([
    "BindFailed", "WriteFailed", "InvalidConfiguration", "PermissionDenied",
    "DeviceUnavailable", "ConnectFailed", "BackendFailed",
  ]);
  const settlements = [
    ...outcomes.map((outcome) => Tag("Succeeded", outcome)),
    ...failureTags.map((tag) => detailFailures.has(tag)
      ? Tag("Failed", Tag(tag, { detail: `${tag} detail` }))
      : Tag("Failed", Tag(tag))),
  ].map((outcome, index) => ({
    id: index + 1,
    call: index % 2 === 0 ? "Execute" : "SendResourceBlob",
    outcome,
  }));
  const encoder = new WireBatchEncoder({ codec: workerSettlementCodec });
  const decoder = new WireBatchDecoder([workerSettlementCodec]);
  const encoded = encoder.encode(settlements);
  assert.equal(encoded.message.tag, "CodecBatch");
  assert.deepEqual(decoder.decode(encoded.message), settlements);
});

test("Prns codecs decline unsupported control calls to native clone", () => {
  const invocation = { id: 1, call: Tag("Snapshot") };
  const encoded = new WireBatchEncoder({ codec: workerInvocationCodec }).encode([invocation]);
  assert.equal(encoded.message.tag, "ClonedBatch");
  assert.deepEqual(encoded.message.data.values, [invocation]);
});

test("the measured Prns codec threshold preserves small native-clone batches", () => {
  const command = Tag("RequestPath", {
    destination: fixedBytes(DESTINATION_HASH_LENGTH, 30),
  });
  const values = Array.from(
    { length: MINIMUM_WORKER_CODEC_ITEMS },
    (_, index) => ({ id: index + 1, call: Tag("Execute", command) }),
  );
  const encoder = new WireBatchEncoder({
    codec: workerInvocationCodec,
    minimumItems: MINIMUM_WORKER_CODEC_ITEMS,
  });
  assert.equal(encoder.encode(values.slice(1)).message.tag, "ClonedBatch");
  assert.equal(encoder.encode(values).message.tag, "CodecBatch");
});

test("Prns codecs reject values outside their declared runtime algebra", () => {
  assert.throws(
    () => workerSettlementCodec.encode([{
      id: 1,
      call: "Execute",
      outcome: Tag("Succeeded", Tag("FutureOutcome")),
    }]),
    /byte value|unsupported outcome/,
  );
  const encoded = workerInvocationCodec.encode([{
    id: 1,
    call: Tag("Execute", Tag("RequestPath", {
      destination: fixedBytes(DESTINATION_HASH_LENGTH, 31),
    })),
  }]);
  new Uint8Array(encoded)[0] ^= 0xff;
  assert.throws(() => workerInvocationCodec.decode(encoded), /unknown format/);
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

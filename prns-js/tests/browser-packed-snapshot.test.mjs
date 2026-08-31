import assert from "node:assert/strict";
import { test } from "node:test";

import { parsePackedSnapshot } from "../dist/browser/packed_snapshot.js";

const interfaceId = Uint8Array.of(23, 2, 3, 4, 5, 6, 7, 8);
const destination = fixedBytes(16, 20);
const viaIdentity = fixedBytes(16, 40);
const knownIdentity = fixedBytes(16, 60);

test("packed snapshots materialize closed vocabulary only at the consumer edge", () => {
  assert.deepEqual(parsePackedSnapshot(snapshotBytes()), {
    type: "snapshot",
    revision: 42n,
    ingestedPackets: 7,
    ingestedCommands: 8,
    routes: 1,
    scheduledAnnounces: 2,
    interfaces: [{
      id: interfaceId,
      kind: "websocket-client",
      bitrateBps: 1_000_000,
      hardwareMtu: 65_536,
      routes: 1,
      links: 1,
      transportedLinks: 1,
    }],
    activeLinkCount: 1,
    routeSnapshots: [{
      destination,
      hops: 3,
      viaIdentity,
      interfaceId,
      learnedAtMillis: 10,
      lastRouteActivityAtMillis: 11,
      expiresAtMillis: 12,
    }],
    destinationIdentities: [{
      destination,
      identity: knownIdentity,
    }],
  });
});

test("packed snapshots reject unknown versions, malformed flags, and trailing data", () => {
  const unknownVersion = snapshotBytes();
  new DataView(unknownVersion.buffer).setUint32(4, 2, true);
  assert.throws(() => parsePackedSnapshot(unknownVersion), /unsupported version/);

  const malformedPresence = snapshotBytes();
  malformedPresence[92] = 2;
  assert.throws(() => parsePackedSnapshot(malformedPresence), /presence code/);

  const trailing = new Uint8Array(snapshotBytes().byteLength + 1);
  trailing.set(snapshotBytes());
  assert.throws(() => parsePackedSnapshot(trailing), /trailing bytes/);
});

function snapshotBytes() {
  const bytes = [];
  pushBytes(bytes, Uint8Array.of(0x50, 0x53, 0x4e, 0x50));
  pushU32(bytes, 1);
  pushU64(bytes, 42);
  pushU64(bytes, 7);
  pushU64(bytes, 8);
  pushU64(bytes, 1);
  pushU64(bytes, 2);
  pushU64(bytes, 1);
  pushU64(bytes, 1);
  pushU64(bytes, 1);
  pushU64(bytes, 1);
  pushBytes(bytes, interfaceId);
  pushU32(bytes, 1_000_000);
  bytes.push(1);
  pushU32(bytes, 65_536);
  pushU64(bytes, 1);
  pushU64(bytes, 1);
  pushU64(bytes, 1);
  pushBytes(bytes, destination);
  bytes.push(3, 1);
  pushBytes(bytes, viaIdentity);
  pushBytes(bytes, interfaceId);
  pushU64(bytes, 10);
  pushU64(bytes, 11);
  pushU64(bytes, 12);
  pushBytes(bytes, destination);
  pushBytes(bytes, knownIdentity);
  return Uint8Array.from(bytes);
}

function fixedBytes(length, seed) {
  return Uint8Array.from({ length }, (_, index) => seed + index);
}

function pushBytes(target, source) {
  target.push(...source);
}

function pushU32(target, value) {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  pushBytes(target, bytes);
}

function pushU64(target, value) {
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setBigUint64(0, BigInt(value), true);
  pushBytes(target, bytes);
}

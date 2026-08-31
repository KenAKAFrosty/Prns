import {
  DESTINATION_HASH_LENGTH,
  IDENTITY_HASH_LENGTH,
  INTERFACE_ID_LENGTH,
  destinationHash,
  identityHash,
  interfaceId,
} from "../contract.js";
import type {
  DestinationIdentitySnapshot,
  RouteSnapshot,
} from "../contract.js";
import type { InterfaceSnapshot, PrnsSnapshot } from "./snapshot.js";
import { bitrateBps, hardwareMtu } from "./values.js";

const PACKED_SNAPSHOT_MAGIC = Object.freeze([0x50, 0x53, 0x4e, 0x50]);
const PACKED_SNAPSHOT_VERSION = 1;
const MAXIMUM_SAFE_INTEGER_BIGINT = BigInt(Number.MAX_SAFE_INTEGER);
const MINIMUM_INTERFACE_BYTES = 37;
const MINIMUM_ROUTE_BYTES = 50;
const DESTINATION_IDENTITY_BYTES = DESTINATION_HASH_LENGTH + IDENTITY_HASH_LENGTH;
const RUNTIME_INTERFACE_KIND_NAMES = Object.freeze([
  "loopback",
  "tcp-client",
  "tcp-server",
  "udp",
  "serial",
  "usb-auto-host",
  "usb-auto-device",
  "auto-wifi",
  "wifi-peer",
  "local-server",
  "local-client",
  "tcp-server-peer",
  "bluetooth-auto",
  "bluetooth-peer",
  "lora",
  "kiss",
  "ax25-kiss",
  "pipe",
  "rnode",
  "backbone-server",
  "backbone-server-peer",
  "backbone-client",
  "esp-now",
  "websocket-client",
  "websocket-server",
  "websocket-server-peer",
  "wifi-direct",
  "wifi-direct-peer",
  "wifi-aware",
  "wifi-aware-peer",
  "i2p",
  "i2p-peer",
  "weave",
  "weave-peer",
] as const);

export function parsePackedSnapshot(bytes: Uint8Array): PrnsSnapshot {
  const reader = new PackedSnapshotReader(bytes);
  reader.magic(PACKED_SNAPSHOT_MAGIC);
  reader.version(PACKED_SNAPSHOT_VERSION);
  const revision = reader.u64();
  const ingestedPackets = reader.safeNumber("ingested packet count");
  const ingestedCommands = reader.safeNumber("ingested command count");
  const routes = reader.safeNumber("route count");
  const scheduledAnnounces = reader.safeNumber("scheduled announce count");
  const activeLinkCount = reader.safeNumber("active link count");
  const interfaceCount = reader.rowCount("interface", MINIMUM_INTERFACE_BYTES);
  const routeSnapshotCount = reader.safeNumber("route snapshot count");
  const destinationIdentityCount = reader.safeNumber("destination identity count");
  const interfaces = new Array<InterfaceSnapshot>(interfaceCount);
  for (let index = 0; index < interfaceCount; index += 1) {
    interfaces[index] = readInterface(reader);
  }
  reader.requireRows("route", routeSnapshotCount, MINIMUM_ROUTE_BYTES);
  const routeSnapshots = new Array<RouteSnapshot>(routeSnapshotCount);
  for (let index = 0; index < routeSnapshotCount; index += 1) {
    routeSnapshots[index] = readRoute(reader);
  }
  reader.requireRows(
    "destination identity",
    destinationIdentityCount,
    DESTINATION_IDENTITY_BYTES,
  );
  const destinationIdentities = new Array<DestinationIdentitySnapshot>(
    destinationIdentityCount,
  );
  for (let index = 0; index < destinationIdentityCount; index += 1) {
    destinationIdentities[index] = {
      destination: destinationHash(reader.bytes(DESTINATION_HASH_LENGTH)),
      identity: identityHash(reader.bytes(IDENTITY_HASH_LENGTH)),
    };
  }
  reader.requireFinished();
  return {
    type: "snapshot",
    revision,
    ingestedPackets,
    ingestedCommands,
    routes,
    scheduledAnnounces,
    interfaces,
    activeLinkCount,
    routeSnapshots,
    destinationIdentities,
  };
}

function readInterface(reader: PackedSnapshotReader): InterfaceSnapshot {
  const id = interfaceId(reader.bytes(INTERFACE_ID_LENGTH));
  const bitrate = bitrateBps(reader.u32());
  const hasHardwareMtu = reader.boolean();
  const hardwareMtuValue = hasHardwareMtu
    ? hardwareMtu(reader.u32())
    : undefined;
  const kindCode = id[0];
  const snapshot: InterfaceSnapshot = {
    id,
    kind: kindCode === undefined
      ? "unknown"
      : RUNTIME_INTERFACE_KIND_NAMES[kindCode] ?? "unknown",
    bitrateBps: bitrate,
    routes: reader.safeNumber("interface route count"),
    links: reader.safeNumber("interface link count"),
    transportedLinks: reader.safeNumber("transported link count"),
  };
  if (hardwareMtuValue !== undefined) {
    snapshot.hardwareMtu = hardwareMtuValue;
  }
  return snapshot;
}

function readRoute(reader: PackedSnapshotReader): RouteSnapshot {
  const destination = destinationHash(reader.bytes(DESTINATION_HASH_LENGTH));
  const hops = reader.u8();
  const hasViaIdentity = reader.boolean();
  const viaIdentity = hasViaIdentity
    ? identityHash(reader.bytes(IDENTITY_HASH_LENGTH))
    : undefined;
  const interfaceIdValue = interfaceId(reader.bytes(INTERFACE_ID_LENGTH));
  const learnedAtMillis = reader.safeNumber("route learned time");
  const lastRouteActivityAtMillis = reader.safeNumber("route activity time");
  const expiresAtMillis = reader.safeNumber("route expiry time");
  return {
    destination,
    hops,
    ...(viaIdentity === undefined ? {} : { viaIdentity }),
    interfaceId: interfaceIdValue,
    learnedAtMillis,
    lastRouteActivityAtMillis,
    expiresAtMillis,
  };
}

class PackedSnapshotReader {
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(bytes: Uint8Array) {
    if (!(bytes instanceof Uint8Array)) {
      throw new TypeError("packed snapshot is not a Uint8Array");
    }
    this.#bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  magic(expected: readonly number[]): void {
    for (const byte of expected) {
      if (this.u8() !== byte) {
        throw new TypeError("packed snapshot has an unknown format");
      }
    }
  }

  version(expected: number): void {
    if (this.u32() !== expected) {
      throw new TypeError("packed snapshot has an unsupported version");
    }
  }

  u8(): number {
    this.#require(1);
    const value = this.#view.getUint8(this.#offset);
    this.#offset += 1;
    return value;
  }

  u32(): number {
    this.#require(4);
    const value = this.#view.getUint32(this.#offset, true);
    this.#offset += 4;
    return value;
  }

  u64(): bigint {
    this.#require(8);
    const value = this.#view.getBigUint64(this.#offset, true);
    this.#offset += 8;
    return value;
  }

  safeNumber(label: string): number {
    const value = this.u64();
    if (value > MAXIMUM_SAFE_INTEGER_BIGINT) {
      throw new TypeError(`packed snapshot ${label} exceeds the safe integer range`);
    }
    return Number(value);
  }

  boolean(): boolean {
    const value = this.u8();
    if (value !== 0 && value !== 1) {
      throw new TypeError("packed snapshot contains an invalid presence code");
    }
    return value === 1;
  }

  bytes(length: number): Uint8Array {
    this.#require(length);
    const value = this.#bytes.subarray(this.#offset, this.#offset + length);
    this.#offset += length;
    return value;
  }

  rowCount(label: string, minimumBytes: number): number {
    const count = this.safeNumber(`${label} count`);
    this.requireRows(label, count, minimumBytes);
    return count;
  }

  requireRows(label: string, count: number, minimumBytes: number): void {
    if (count > Math.floor((this.#bytes.byteLength - this.#offset) / minimumBytes)) {
      throw new TypeError(`packed snapshot ${label} count exceeds its payload`);
    }
  }

  requireFinished(): void {
    if (this.#offset !== this.#bytes.byteLength) {
      throw new TypeError("packed snapshot contains trailing bytes");
    }
  }

  #require(length: number): void {
    if (
      !Number.isSafeInteger(length) ||
      length < 0 ||
      length > this.#bytes.byteLength - this.#offset
    ) {
      throw new TypeError("packed snapshot is truncated");
    }
  }
}

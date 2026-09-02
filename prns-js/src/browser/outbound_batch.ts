import { Tag } from "../casework.js";
import { interfaceId } from "../contract.js";
import { INTERFACE_ID_LENGTH } from "../contract.generated.js";
import { runtimeInterfaceKindFromCode } from "./interface_kind.js";
import type {
  FanTarget,
  OutboundTarget,
  PrnsOutboundFrame,
} from "./outbound.js";
import { hopCount, packetFrameView } from "./values.js";

const OUTBOUND_BATCH_MAGIC = 0x5455_4f50;
const OUTBOUND_BATCH_VERSION = 1;
const OUTBOUND_BATCH_MINIMUM_RECORD_BYTES = 8;

export function parseOutboundBatch(bytes: Uint8Array): PrnsOutboundFrame[] {
  const reader = new OutboundBatchReader(bytes);
  if (reader.u32() !== OUTBOUND_BATCH_MAGIC) {
    throw new TypeError("outbound batch magic is invalid");
  }
  if (reader.u16() !== OUTBOUND_BATCH_VERSION) {
    throw new TypeError("outbound batch version is unsupported");
  }
  if (reader.u16() !== 0) {
    throw new TypeError("outbound batch reserved bits are set");
  }
  const count = reader.u32();
  if (count > Math.floor(reader.remaining / OUTBOUND_BATCH_MINIMUM_RECORD_BYTES)) {
    throw new TypeError("outbound batch record count exceeds its bytes");
  }
  const frames = new Array<PrnsOutboundFrame>(count);
  for (let index = 0; index < count; index += 1) {
    const kind = reader.u8();
    const type = kind === 0 ? "frame" : kind === 1 ? "announce" : undefined;
    if (type === undefined) {
      throw new TypeError("outbound batch frame kind is unknown");
    }
    const hops = type === "announce" ? hopCount(reader.u8()) : undefined;
    const target = readTarget(reader);
    const bytes = packetFrameView(reader.bytes(reader.u32()));
    frames[index] = {
      type,
      target,
      ...(hops === undefined ? {} : { hops }),
      bytes,
    };
  }
  reader.requireFinished();
  return frames;
}

function readTarget(reader: OutboundBatchReader): OutboundTarget {
  const kind = reader.u8();
  if (kind === 0) {
    return Tag("Interface", interfaceId(reader.bytes(INTERFACE_ID_LENGTH)));
  }
  if (kind !== 1) {
    throw new TypeError("outbound batch target kind is unknown");
  }
  return Tag("Broadcast", {
    supervisorKind: runtimeInterfaceKindFromCode(reader.u8()),
    fan: readFanTarget(reader),
  });
}

function readFanTarget(reader: OutboundBatchReader): FanTarget {
  const kind = reader.u8();
  if (kind === 0) {
    return Tag("All");
  }
  if (kind !== 1 && kind !== 2) {
    throw new TypeError("outbound batch fan target is unknown");
  }
  const target = interfaceId(reader.bytes(INTERFACE_ID_LENGTH));
  if (kind === 1) {
    return Tag("Only", target);
  }
  return Tag("AllExcept", target);
}

class OutboundBatchReader {
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(bytes: Uint8Array) {
    if (!(bytes instanceof Uint8Array)) {
      throw new TypeError("outbound batch is not a Uint8Array");
    }
    this.#bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  get remaining(): number {
    return this.#bytes.byteLength - this.#offset;
  }

  u8(): number {
    this.#require(1);
    const value = this.#view.getUint8(this.#offset);
    this.#offset += 1;
    return value;
  }

  u16(): number {
    this.#require(2);
    const value = this.#view.getUint16(this.#offset, true);
    this.#offset += 2;
    return value;
  }

  u32(): number {
    this.#require(4);
    const value = this.#view.getUint32(this.#offset, true);
    this.#offset += 4;
    return value;
  }

  bytes(length: number): Uint8Array {
    this.#require(length);
    const value = this.#bytes.subarray(this.#offset, this.#offset + length);
    this.#offset += length;
    return value;
  }

  requireFinished(): void {
    if (this.remaining !== 0) {
      throw new TypeError("outbound batch contains trailing bytes");
    }
  }

  #require(length: number): void {
    if (!Number.isSafeInteger(length) || length < 0 || length > this.remaining) {
      throw new TypeError("outbound batch is truncated");
    }
  }
}

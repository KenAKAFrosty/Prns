import { Tag } from "../casework.js";
import { commandId } from "./values.js";
import {
  interfaceId,
  linkId,
  packetHash,
} from "../contract.js";
import type {
  CommandFailure,
  CommandOutcome,
  CommandSettlement,
  DeliveryEvidenceKind,
} from "../contract.js";
import {
  COMMAND_FAILURE_KIND_CODES,
  COMMAND_OUTCOME_KIND_CODES,
  DELIVERY_EVIDENCE_KIND_CODES,
  HOST_SCHEMA_VERSION,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  PACKET_HASH_LENGTH,
} from "../contract.generated.js";
import type { CommandId } from "./values.js";

const COMMAND_SETTLEMENT_BATCH_MAGIC = 0x4353_5250;
const COMMAND_SETTLEMENT_BATCH_FORMAT_VERSION = 1;
const COMMAND_SETTLEMENT_BATCH_HEADER_BYTES = 16;
const COMMAND_SETTLEMENT_MINIMUM_RECORD_BYTES = 9;
const MAXIMUM_SAFE_INTEGER_BIGINT = BigInt(Number.MAX_SAFE_INTEGER);
const textDecoder = new TextDecoder("utf-8", { fatal: true });

type CommandSettledEvent = Tag<
  "CommandSettled",
  {
    readonly commandId: CommandId;
    readonly settlement?: CommandSettlement;
  }
>;

const FAILURE_TAGS = invertCodes(COMMAND_FAILURE_KIND_CODES);
const DETAIL_FAILURES = new Set<CommandFailure["tag"]>([
  "BindFailed",
  "WriteFailed",
  "InvalidConfiguration",
  "PermissionDenied",
  "DeviceUnavailable",
  "ConnectFailed",
  "BackendFailed",
]);

export function parseCommandSettlementBatch(
  bytes: Uint8Array,
): CommandSettledEvent[] {
  const reader = new SettlementReader(bytes);
  if (reader.u32() !== COMMAND_SETTLEMENT_BATCH_MAGIC) {
    throw new TypeError("command settlement batch magic is invalid");
  }
  if (reader.u16() !== COMMAND_SETTLEMENT_BATCH_FORMAT_VERSION) {
    throw new TypeError("command settlement batch version is unsupported");
  }
  if (reader.u16() !== 0) {
    throw new TypeError("command settlement batch reserved bits are set");
  }
  if (reader.u32() !== HOST_SCHEMA_VERSION) {
    throw new TypeError("command settlement batch schema is incompatible");
  }
  const count = reader.u32();
  if (count > Math.floor(reader.remaining / COMMAND_SETTLEMENT_MINIMUM_RECORD_BYTES)) {
    throw new TypeError("command settlement batch record count exceeds its bytes");
  }
  const events = new Array<CommandSettledEvent>(count);
  for (let index = 0; index < count; index += 1) {
    const id = commandId(reader.u64());
    const result = reader.u8();
    if (result === 2) {
      events[index] = Tag("CommandSettled", { commandId: id });
      continue;
    }
    const settlement = result === 0
      ? Tag("Succeeded", decodeOutcome(reader))
      : result === 1
        ? Tag("Failed", decodeFailure(reader))
        : undefined;
    if (settlement === undefined) {
      throw new TypeError("command settlement batch result is unknown");
    }
    events[index] = Tag("CommandSettled", {
      commandId: id,
      settlement,
    });
  }
  reader.requireFinished();
  return events;
}

function decodeOutcome(reader: SettlementReader): CommandOutcome {
  const code = reader.u32();
  switch (code) {
    case COMMAND_OUTCOME_KIND_CODES.Announced:
      return Tag("Announced");
    case COMMAND_OUTCOME_KIND_CODES.PacketDelivered: {
      const rttMillis = reader.safeUint();
      const evidence = decodeEvidence(reader.u32());
      const hasPacketHash = reader.boolean();
      const hash = hasPacketHash ? packetHash(reader.fixed(PACKET_HASH_LENGTH)) : undefined;
      return Tag(
        "PacketDelivered",
        hash === undefined ? { rttMillis, evidence } : { rttMillis, evidence, packetHash: hash },
      );
    }
    case COMMAND_OUTCOME_KIND_CODES.LinkCloseQueued:
      return Tag("LinkCloseQueued");
    case COMMAND_OUTCOME_KIND_CODES.InterfaceAttached:
      return Tag("InterfaceAttached", {
        interface: interfaceId(reader.fixed(INTERFACE_ID_LENGTH)),
      });
    case COMMAND_OUTCOME_KIND_CODES.InterfaceDetached:
      return Tag("InterfaceDetached", {
        interface: interfaceId(reader.fixed(INTERFACE_ID_LENGTH)),
      });
    case COMMAND_OUTCOME_KIND_CODES.LinkEstablished:
      return Tag("LinkEstablished", {
        linkId: linkId(reader.fixed(LINK_ID_LENGTH)),
        rttMillis: reader.safeUint(),
      });
    case COMMAND_OUTCOME_KIND_CODES.PathDiscovered:
      return Tag("PathDiscovered", { hops: reader.safeUint() });
    case COMMAND_OUTCOME_KIND_CODES.Identified:
      return Tag("Identified");
    case COMMAND_OUTCOME_KIND_CODES.ResponseReceived:
      return Tag("ResponseReceived", {
        data: reader.bytes(),
        rttMillis: reader.safeUint(),
      });
    case COMMAND_OUTCOME_KIND_CODES.ResponseSent:
      return Tag("ResponseSent", { rttMillis: reader.safeUint() });
    case COMMAND_OUTCOME_KIND_CODES.ResourceSent:
      return Tag("ResourceSent");
    case COMMAND_OUTCOME_KIND_CODES.ResourceStrategySet:
      return Tag("ResourceStrategySet");
    case COMMAND_OUTCOME_KIND_CODES.RequesterAllowed:
      return Tag("RequesterAllowed");
    default:
      throw new TypeError("command settlement batch outcome is unknown");
  }
}

function decodeFailure(reader: SettlementReader): CommandFailure {
  const code = reader.u32();
  const tag = FAILURE_TAGS.get(code);
  if (tag === undefined) {
    throw new TypeError("command settlement batch failure is unknown");
  }
  return DETAIL_FAILURES.has(tag)
    ? Tag(tag, { detail: reader.string() }) as CommandFailure
    : Tag(tag) as CommandFailure;
}

function decodeEvidence(code: number): DeliveryEvidenceKind {
  switch (code) {
    case DELIVERY_EVIDENCE_KIND_CODES.ExplicitProof:
      return "ExplicitProof";
    case DELIVERY_EVIDENCE_KIND_CODES.ImplicitProof:
      return "ImplicitProof";
    case DELIVERY_EVIDENCE_KIND_CODES.Response:
      return "Response";
    default:
      throw new TypeError("command settlement batch delivery evidence is unknown");
  }
}

function invertCodes<Codes extends Readonly<Record<string, number>>>(
  codes: Codes,
): ReadonlyMap<number, keyof Codes & string> {
  return new Map(
    Object.entries(codes).map(([tag, code]) => [code, tag as keyof Codes & string]),
  );
}

class SettlementReader {
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(bytes: Uint8Array) {
    if (!(bytes instanceof Uint8Array) || bytes.byteLength < COMMAND_SETTLEMENT_BATCH_HEADER_BYTES) {
      throw new TypeError("command settlement batch is truncated");
    }
    this.#bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  get remaining(): number {
    return this.#bytes.byteLength - this.#offset;
  }

  u8(): number {
    this.#require(1);
    return this.#view.getUint8(this.#offset++);
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

  u64(): bigint {
    this.#require(8);
    const value = this.#view.getBigUint64(this.#offset, true);
    this.#offset += 8;
    return value;
  }

  safeUint(): number {
    const value = this.u64();
    if (value > MAXIMUM_SAFE_INTEGER_BIGINT) {
      throw new TypeError("command settlement integer exceeds the safe-integer limit");
    }
    return Number(value);
  }

  boolean(): boolean {
    const value = this.u8();
    if (value === 0) {
      return false;
    }
    if (value === 1) {
      return true;
    }
    throw new TypeError("command settlement batch boolean is invalid");
  }

  fixed(length: number): Uint8Array {
    this.#require(length);
    const value = this.#bytes.subarray(this.#offset, this.#offset + length);
    this.#offset += length;
    return value;
  }

  bytes(): Uint8Array {
    return this.fixed(this.u32()).slice();
  }

  string(): string {
    return textDecoder.decode(this.fixed(this.u32()));
  }

  requireFinished(): void {
    if (this.remaining !== 0) {
      throw new TypeError("command settlement batch contains trailing bytes");
    }
  }

  #require(length: number): void {
    if (!Number.isSafeInteger(length) || length < 0 || length > this.remaining) {
      throw new TypeError("command settlement batch is truncated");
    }
  }
}

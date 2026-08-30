import { Tag } from "../casework.js";
import {
  DESTINATION_HASH_LENGTH,
  IDENTITY_HASH_LENGTH,
  INTERFACE_ID_LENGTH,
  LINK_ID_LENGTH,
  PACKET_HASH_LENGTH,
  REQUEST_ID_LENGTH,
  REQUEST_PATH_HASH_LENGTH,
} from "../contract.js";
import type {
  CommandFailure,
  CommandOutcome,
  CommandSettlement,
  DeliveryEvidenceKind,
  DestinationHash,
  HostCommand,
  IdentityHash,
  InterfaceId,
  LinkId,
  PacketHash,
  RequestId,
  RequestPathHash,
  ResourceCompression,
  ResourceStrategy,
  ResponseTimeout,
} from "../contract.js";
import {
  MAXIMUM_WIRE_BATCH_ITEMS,
} from "../worker_wire/wire_batch.js";
import type { WireCodec } from "../worker_wire/wire_batch.js";
import type { WorkerInvocation, WorkerSettlement } from "./worker_protocol.js";

type EncodedHostCommand = Exclude<
  HostCommand,
  | { readonly tag: "AttachTcpServer" }
  | { readonly tag: "AttachTcpClient" }
  | { readonly tag: "AttachUdp" }
  | { readonly tag: "AttachInterface" }
>;

const WORKER_INVOCATION_MAGIC = 0x5052_4951;
const WORKER_SETTLEMENT_MAGIC = 0x5052_5351;
export const MINIMUM_WORKER_CODEC_ITEMS = 10;

const SETTLEMENT_CALL_CODES = {
  Execute: 0,
  SendResourceBlob: 1,
  Snapshot: 2,
} as const;

const COMMAND_CODES = {
  Announce: 0,
  SendSinglePacket: 1,
  CloseLink: 2,
  DetachInterface: 3,
  EstablishLink: 4,
  RequestPath: 5,
  Identify: 6,
  SendLinkPacket: 7,
  Request: 8,
  Respond: 9,
  SendResource: 10,
  SetLinkResourceStrategy: 11,
  SetDestinationResourceStrategy: 12,
  SendChannelMessage: 13,
  AllowRequester: 14,
} as const satisfies Record<EncodedHostCommand["tag"], number>;

const OUTCOME_CODES = {
  Announced: 0,
  PacketDelivered: 1,
  LinkCloseQueued: 2,
  InterfaceAttached: 3,
  InterfaceDetached: 4,
  LinkEstablished: 5,
  PathDiscovered: 6,
  Identified: 7,
  ResponseReceived: 8,
  ResponseSent: 9,
  ResourceSent: 10,
  ResourceStrategySet: 11,
  RequesterAllowed: 12,
} as const satisfies Record<CommandOutcome["tag"], number>;

type FailureFormats = {
  readonly [Name in CommandFailure["tag"]]: {
    readonly code: number;
    readonly data: Extract<CommandFailure, { readonly tag: Name }>["data"] extends undefined
      ? "Unit"
      : "Detail";
  };
};

const FAILURE_FORMATS = {
  NodeStopped: { code: 0, data: "Unit" },
  Busy: { code: 1, data: "Unit" },
  PayloadTooLarge: { code: 2, data: "Unit" },
  UnknownDestination: { code: 3, data: "Unit" },
  NotSingleDestination: { code: 4, data: "Unit" },
  AnnounceAppDataTooLong: { code: 5, data: "Unit" },
  UnknownInterface: { code: 6, data: "Unit" },
  NoRouteToDestination: { code: 7, data: "Unit" },
  NotDirectlyReachable: { code: 8, data: "Unit" },
  PacketCulled: { code: 9, data: "Unit" },
  DeliveryTimedOut: { code: 10, data: "Unit" },
  InvalidBitrate: { code: 11, data: "Unit" },
  BindFailed: { code: 12, data: "Detail" },
  WriteFailed: { code: 13, data: "Detail" },
  UnsupportedByBackend: { code: 14, data: "Unit" },
  UnknownLink: { code: 15, data: "Unit" },
  LinkNotActive: { code: 16, data: "Unit" },
  EntropyUnavailable: { code: 17, data: "Unit" },
  NotLinkInitiator: { code: 18, data: "Unit" },
  IdentityNotHeld: { code: 19, data: "Unit" },
  UnknownRequestHandler: { code: 20, data: "Unit" },
  RequestPolicyNotAllowList: { code: 21, data: "Unit" },
  RequestAllowListFull: { code: 22, data: "Unit" },
  LinkBusy: { code: 23, data: "Unit" },
  ResourceTableFull: { code: 24, data: "Unit" },
  ResourceMetadataTooLarge: { code: 25, data: "Unit" },
  ResourceRejectedByPeer: { code: 26, data: "Unit" },
  ResourceSequencingFailed: { code: 27, data: "Unit" },
  ResourcePredecessorFailed: { code: 28, data: "Unit" },
  ChannelWindowFull: { code: 29, data: "Unit" },
  ChannelUntrackable: { code: 30, data: "Unit" },
  InvalidChannelMessageType: { code: 31, data: "Unit" },
  InvalidConfiguration: { code: 32, data: "Detail" },
  ResourceUploadCancelled: { code: 33, data: "Unit" },
  ResourceEarlyEof: { code: 34, data: "Unit" },
  ResourceLengthOverrun: { code: 35, data: "Unit" },
  PermissionDenied: { code: 36, data: "Detail" },
  DeviceUnavailable: { code: 37, data: "Detail" },
  ConnectFailed: { code: 38, data: "Detail" },
  BackendFailed: { code: 39, data: "Detail" },
  ResponseTooLarge: { code: 40, data: "Unit" },
  LinkClosed: { code: 41, data: "Unit" },
  ResponseCancelledBySender: { code: 42, data: "Unit" },
  ResponseHashmapBeyondPartCount: { code: 43, data: "Unit" },
  ResponseHashmapSkipsAhead: { code: 44, data: "Unit" },
  ResponseHashmapTooLong: { code: 45, data: "Unit" },
  ResponseHashmapRagged: { code: 46, data: "Unit" },
  ResponseRetriesExhausted: { code: 47, data: "Unit" },
  ResponseLinkVanished: { code: 48, data: "Unit" },
  ResponseTransferUnopenable: { code: 49, data: "Unit" },
  ResponseTransferCorrupt: { code: 50, data: "Unit" },
  ResponseProofUnsendable: { code: 51, data: "Unit" },
  ResponseDecompressionFailed: { code: 52, data: "Unit" },
  ResponseDecompressionTimedOut: { code: 53, data: "Unit" },
  ResponseOpenTimedOut: { code: 54, data: "Unit" },
  ResponseMetadataOverrun: { code: 55, data: "Unit" },
} as const satisfies FailureFormats;

const COMMAND_TAGS = invertCodes(COMMAND_CODES, "worker command");
const OUTCOME_TAGS = invertCodes(OUTCOME_CODES, "command outcome");
const FAILURE_TAGS = invertFormats(FAILURE_FORMATS, "command failure");
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true });

export const workerInvocationCodec: WireCodec<WorkerInvocation> = {
  id: "prns-worker-invocation-v1",
  accepts: (values) => values.every(isEncodableInvocation),
  encode: encodeInvocations,
  decode: decodeInvocations,
};

export const workerSettlementCodec: WireCodec<WorkerSettlement> = {
  id: "prns-worker-settlement-v1",
  accepts: (values) => values.every(isEncodableSettlement),
  encode: encodeSettlements,
  decode: decodeSettlements,
};

export function workerInvocationWireBytes(value: WorkerInvocation): number {
  if (isEncodableInvocation(value)) {
    return 16 + encodedCommandSize(value.call.data as EncodedHostCommand);
  }
  if (value.call.tag === "RegisterNodePage") {
    return 32 + value.call.data.byteLength;
  }
  return 256;
}

export function workerSettlementWireBytes(value: WorkerSettlement): number {
  if (isPackedSnapshotSettlement(value)) {
    return 17 + bytesSize(value.outcome.data);
  }
  if (isEncodableSettlement(value)) {
    return 17 + encodedSettlementSize(value.outcome as CommandSettlement);
  }
  return 256;
}

function isEncodableInvocation(value: WorkerInvocation): boolean {
  return Number.isSafeInteger(value.id) && value.id > 0 &&
    value.call.tag === "Execute" && isEncodedCommand(value.call.data);
}

function isEncodableSettlement(value: WorkerSettlement): boolean {
  if (!Number.isSafeInteger(value.id) || value.id <= 0) {
    return false;
  }
  return isPackedSnapshotSettlement(value) ||
    ((value.call === "Execute" || value.call === "SendResourceBlob") &&
      isCommandSettlement(value.outcome));
}

function isPackedSnapshotSettlement(
  value: WorkerSettlement,
): value is WorkerSettlement & {
  readonly call: "Snapshot";
  readonly outcome: { readonly tag: "PackedSnapshot"; readonly data: Uint8Array };
} {
  return value.call === "Snapshot" &&
    typeof value.outcome === "object" &&
    value.outcome !== null &&
    (value.outcome as { readonly tag?: unknown }).tag === "PackedSnapshot" &&
    (value.outcome as { readonly data?: unknown }).data instanceof Uint8Array;
}

function isEncodedCommand(value: HostCommand): value is EncodedHostCommand {
  return Object.hasOwn(COMMAND_CODES, value.tag);
}

function isCommandSettlement(value: unknown): value is CommandSettlement {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const settlement = value as { readonly tag?: unknown; readonly data?: unknown };
  if (typeof settlement.data !== "object" || settlement.data === null) {
    return false;
  }
  const outcomeTag = (settlement.data as { readonly tag?: unknown }).tag;
  return (settlement.tag === "Succeeded" &&
      typeof outcomeTag === "string" && Object.hasOwn(OUTCOME_CODES, outcomeTag)) ||
    (settlement.tag === "Failed" &&
      typeof outcomeTag === "string" && Object.hasOwn(FAILURE_FORMATS, outcomeTag));
}

function encodeInvocations(values: readonly WorkerInvocation[]): ArrayBuffer {
  requireItemCount(values.length);
  let byteLength = 8;
  for (const value of values) {
    if (!isEncodableInvocation(value)) {
      throw new TypeError("worker invocation codec received an unsupported call");
    }
    byteLength = addSize(
      byteLength,
      8 + encodedCommandSize(value.call.data as EncodedHostCommand),
    );
  }
  const writer = new WireWriter(byteLength);
  writer.u32(WORKER_INVOCATION_MAGIC);
  writer.u32(values.length);
  for (const value of values) {
    writer.f64(value.id);
    encodeCommand(value.call.data as EncodedHostCommand, writer);
  }
  return writer.finish();
}

function decodeInvocations(buffer: ArrayBuffer): readonly WorkerInvocation[] {
  const reader = new WireReader(buffer);
  reader.magic(WORKER_INVOCATION_MAGIC);
  const count = reader.count();
  const values: WorkerInvocation[] = new Array(count);
  for (let index = 0; index < count; index += 1) {
    const id = reader.safeId();
    values[index] = {
      id,
      call: Tag("Execute", decodeCommand(reader)),
    };
  }
  reader.requireFinished();
  return values;
}

function encodedCommandSize(command: EncodedHostCommand): number {
  switch (command.tag) {
    case "Announce":
      return 1 + DESTINATION_HASH_LENGTH + 1 +
        (command.data.interface === undefined ? 0 : INTERFACE_ID_LENGTH);
    case "SendSinglePacket":
      return 1 + DESTINATION_HASH_LENGTH + bytesSize(command.data.payload);
    case "CloseLink":
      return 1 + LINK_ID_LENGTH;
    case "DetachInterface":
      return 1 + INTERFACE_ID_LENGTH;
    case "EstablishLink":
    case "RequestPath":
      return 1 + DESTINATION_HASH_LENGTH;
    case "Identify":
      return 1 + LINK_ID_LENGTH + IDENTITY_HASH_LENGTH;
    case "SendLinkPacket":
      return 1 + LINK_ID_LENGTH + bytesSize(command.data.payload);
    case "Request":
      return 1 + LINK_ID_LENGTH + REQUEST_PATH_HASH_LENGTH +
        bytesSize(command.data.payload) + responseTimeoutSize(command.data.timeout) + 1 +
        (command.data.maximumResponseBytes === undefined ? 0 : 8);
    case "Respond":
      return 1 + LINK_ID_LENGTH + REQUEST_ID_LENGTH + 8 + bytesSize(command.data.payload);
    case "SendResource":
      return 1 + LINK_ID_LENGTH + bytesSize(command.data.payload) + 1 +
        (command.data.packedMetadata === undefined ? 0 : bytesSize(command.data.packedMetadata)) +
        1;
    case "SetLinkResourceStrategy":
      return 1 + LINK_ID_LENGTH + resourceStrategySize(command.data.strategy);
    case "SetDestinationResourceStrategy":
      return 1 + DESTINATION_HASH_LENGTH + resourceStrategySize(command.data.strategy);
    case "SendChannelMessage":
      return 1 + LINK_ID_LENGTH + 8 + bytesSize(command.data.payload);
    case "AllowRequester":
      return 1 + DESTINATION_HASH_LENGTH + REQUEST_PATH_HASH_LENGTH + IDENTITY_HASH_LENGTH;
  }
}

function encodeCommand(command: EncodedHostCommand, writer: WireWriter): void {
  writer.u8(COMMAND_CODES[command.tag]);
  switch (command.tag) {
    case "Announce":
      writer.fixed(command.data.destination, DESTINATION_HASH_LENGTH, "destination hash");
      writer.optionalFixed(command.data.interface, INTERFACE_ID_LENGTH, "interface id");
      return;
    case "SendSinglePacket":
      writer.fixed(command.data.destination, DESTINATION_HASH_LENGTH, "destination hash");
      writer.bytes(command.data.payload);
      return;
    case "CloseLink":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      return;
    case "DetachInterface":
      writer.fixed(command.data.interface, INTERFACE_ID_LENGTH, "interface id");
      return;
    case "EstablishLink":
    case "RequestPath":
      writer.fixed(command.data.destination, DESTINATION_HASH_LENGTH, "destination hash");
      return;
    case "Identify":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      writer.fixed(command.data.identity, IDENTITY_HASH_LENGTH, "identity hash");
      return;
    case "SendLinkPacket":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      writer.bytes(command.data.payload);
      return;
    case "Request":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      writer.fixed(command.data.pathHash, REQUEST_PATH_HASH_LENGTH, "request path hash");
      writer.bytes(command.data.payload);
      encodeResponseTimeout(command.data.timeout, writer);
      writer.optionalNumber(command.data.maximumResponseBytes);
      return;
    case "Respond":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      writer.fixed(command.data.requestId, REQUEST_ID_LENGTH, "request id");
      writer.f64(command.data.requestRttMillis);
      writer.bytes(command.data.payload);
      return;
    case "SendResource":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      writer.bytes(command.data.payload);
      writer.optionalBytes(command.data.packedMetadata);
      encodeResourceCompression(command.data.compression, writer);
      return;
    case "SetLinkResourceStrategy":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      encodeResourceStrategy(command.data.strategy, writer);
      return;
    case "SetDestinationResourceStrategy":
      writer.fixed(command.data.destination, DESTINATION_HASH_LENGTH, "destination hash");
      encodeResourceStrategy(command.data.strategy, writer);
      return;
    case "SendChannelMessage":
      writer.fixed(command.data.linkId, LINK_ID_LENGTH, "link id");
      writer.f64(command.data.messageType);
      writer.bytes(command.data.payload);
      return;
    case "AllowRequester":
      writer.fixed(command.data.destination, DESTINATION_HASH_LENGTH, "destination hash");
      writer.fixed(command.data.pathHash, REQUEST_PATH_HASH_LENGTH, "request path hash");
      writer.fixed(command.data.identity, IDENTITY_HASH_LENGTH, "identity hash");
      return;
  }
}

function decodeCommand(reader: WireReader): EncodedHostCommand {
  const tag = reader.code(COMMAND_TAGS, "worker command");
  switch (tag) {
    case "Announce": {
      const destination = reader.fixed(DESTINATION_HASH_LENGTH) as DestinationHash;
      const interfaceId = reader.optionalFixed(INTERFACE_ID_LENGTH) as InterfaceId | undefined;
      return Tag("Announce", interfaceId === undefined
        ? { destination }
        : { destination, interface: interfaceId });
    }
    case "SendSinglePacket":
      return Tag("SendSinglePacket", {
        destination: reader.fixed(DESTINATION_HASH_LENGTH) as DestinationHash,
        payload: reader.bytes(),
      });
    case "CloseLink":
      return Tag("CloseLink", { linkId: reader.fixed(LINK_ID_LENGTH) as LinkId });
    case "DetachInterface":
      return Tag("DetachInterface", {
        interface: reader.fixed(INTERFACE_ID_LENGTH) as InterfaceId,
      });
    case "EstablishLink":
      return Tag("EstablishLink", {
        destination: reader.fixed(DESTINATION_HASH_LENGTH) as DestinationHash,
      });
    case "RequestPath":
      return Tag("RequestPath", {
        destination: reader.fixed(DESTINATION_HASH_LENGTH) as DestinationHash,
      });
    case "Identify":
      return Tag("Identify", {
        linkId: reader.fixed(LINK_ID_LENGTH) as LinkId,
        identity: reader.fixed(IDENTITY_HASH_LENGTH) as IdentityHash,
      });
    case "SendLinkPacket":
      return Tag("SendLinkPacket", {
        linkId: reader.fixed(LINK_ID_LENGTH) as LinkId,
        payload: reader.bytes(),
      });
    case "Request": {
      const linkId = reader.fixed(LINK_ID_LENGTH) as LinkId;
      const pathHash = reader.fixed(REQUEST_PATH_HASH_LENGTH) as RequestPathHash;
      const payload = reader.bytes();
      const timeout = decodeResponseTimeout(reader);
      const maximumResponseBytes = reader.optionalNumber();
      return Tag("Request", maximumResponseBytes === undefined
        ? { linkId, pathHash, payload, timeout }
        : { linkId, pathHash, payload, timeout, maximumResponseBytes });
    }
    case "Respond":
      return Tag("Respond", {
        linkId: reader.fixed(LINK_ID_LENGTH) as LinkId,
        requestId: reader.fixed(REQUEST_ID_LENGTH) as RequestId,
        requestRttMillis: reader.f64(),
        payload: reader.bytes(),
      });
    case "SendResource": {
      const linkId = reader.fixed(LINK_ID_LENGTH) as LinkId;
      const payload = reader.bytes();
      const packedMetadata = reader.optionalBytes();
      const compression = decodeResourceCompression(reader);
      return Tag("SendResource", packedMetadata === undefined
        ? { linkId, payload, compression }
        : { linkId, payload, packedMetadata, compression });
    }
    case "SetLinkResourceStrategy":
      return Tag("SetLinkResourceStrategy", {
        linkId: reader.fixed(LINK_ID_LENGTH) as LinkId,
        strategy: decodeResourceStrategy(reader),
      });
    case "SetDestinationResourceStrategy":
      return Tag("SetDestinationResourceStrategy", {
        destination: reader.fixed(DESTINATION_HASH_LENGTH) as DestinationHash,
        strategy: decodeResourceStrategy(reader),
      });
    case "SendChannelMessage":
      return Tag("SendChannelMessage", {
        linkId: reader.fixed(LINK_ID_LENGTH) as LinkId,
        messageType: reader.f64(),
        payload: reader.bytes(),
      });
    case "AllowRequester":
      return Tag("AllowRequester", {
        destination: reader.fixed(DESTINATION_HASH_LENGTH) as DestinationHash,
        pathHash: reader.fixed(REQUEST_PATH_HASH_LENGTH) as RequestPathHash,
        identity: reader.fixed(IDENTITY_HASH_LENGTH) as IdentityHash,
      });
  }
}

function encodeSettlements(values: readonly WorkerSettlement[]): ArrayBuffer {
  requireItemCount(values.length);
  let byteLength = 8;
  for (const value of values) {
    if (!isEncodableSettlement(value)) {
      throw new TypeError("worker settlement codec received an unsupported outcome");
    }
    byteLength = addSize(
      byteLength,
      8 + 1 + (isPackedSnapshotSettlement(value)
        ? bytesSize(value.outcome.data)
        : encodedSettlementSize(value.outcome as CommandSettlement)),
    );
  }
  const writer = new WireWriter(byteLength);
  writer.u32(WORKER_SETTLEMENT_MAGIC);
  writer.u32(values.length);
  for (const value of values) {
    writer.f64(value.id);
    if (isPackedSnapshotSettlement(value)) {
      writer.u8(SETTLEMENT_CALL_CODES.Snapshot);
      writer.bytes(value.outcome.data);
      continue;
    }
    writer.u8(SETTLEMENT_CALL_CODES[value.call as "Execute" | "SendResourceBlob"]);
    encodeSettlement(value.outcome as CommandSettlement, writer);
  }
  return writer.finish();
}

function decodeSettlements(buffer: ArrayBuffer): readonly WorkerSettlement[] {
  const reader = new WireReader(buffer);
  reader.magic(WORKER_SETTLEMENT_MAGIC);
  const count = reader.count();
  const values: WorkerSettlement[] = new Array(count);
  for (let index = 0; index < count; index += 1) {
    const id = reader.safeId();
    const callCode = reader.u8();
    if (callCode === SETTLEMENT_CALL_CODES.Execute) {
      values[index] = { id, call: "Execute", outcome: decodeSettlement(reader) };
      continue;
    }
    if (callCode === SETTLEMENT_CALL_CODES.SendResourceBlob) {
      values[index] = { id, call: "SendResourceBlob", outcome: decodeSettlement(reader) };
      continue;
    }
    if (callCode === SETTLEMENT_CALL_CODES.Snapshot) {
      values[index] = {
        id,
        call: "Snapshot",
        outcome: Tag("PackedSnapshot", reader.bytes()),
      };
      continue;
    }
    throw new TypeError("worker settlement contains an unknown call code");
  }
  reader.requireFinished();
  return values;
}

function encodedSettlementSize(settlement: CommandSettlement): number {
  return 1 + (settlement.tag === "Succeeded"
    ? encodedOutcomeSize(settlement.data)
    : encodedFailureSize(settlement.data));
}

function encodeSettlement(settlement: CommandSettlement, writer: WireWriter): void {
  if (settlement.tag === "Succeeded") {
    writer.u8(0);
    encodeOutcome(settlement.data, writer);
    return;
  }
  writer.u8(1);
  encodeFailure(settlement.data, writer);
}

function decodeSettlement(reader: WireReader): CommandSettlement {
  const code = reader.u8();
  if (code === 0) {
    return Tag("Succeeded", decodeOutcome(reader));
  }
  if (code === 1) {
    return Tag("Failed", decodeFailure(reader));
  }
  throw new TypeError("worker settlement contains an unknown result code");
}

function encodedOutcomeSize(outcome: CommandOutcome): number {
  switch (outcome.tag) {
    case "Announced":
    case "LinkCloseQueued":
    case "Identified":
    case "ResourceSent":
    case "ResourceStrategySet":
    case "RequesterAllowed":
      return 1;
    case "PacketDelivered":
      return 1 + 8 + 1 + 1 +
        (outcome.data.packetHash === undefined ? 0 : PACKET_HASH_LENGTH);
    case "InterfaceAttached":
    case "InterfaceDetached":
      return 1 + INTERFACE_ID_LENGTH;
    case "LinkEstablished":
      return 1 + LINK_ID_LENGTH + 8;
    case "PathDiscovered":
    case "ResponseSent":
      return 1 + 8;
    case "ResponseReceived":
      return 1 + bytesSize(outcome.data.data) + 8;
  }
}

function encodeOutcome(outcome: CommandOutcome, writer: WireWriter): void {
  writer.u8(OUTCOME_CODES[outcome.tag]);
  switch (outcome.tag) {
    case "Announced":
    case "LinkCloseQueued":
    case "Identified":
    case "ResourceSent":
    case "ResourceStrategySet":
    case "RequesterAllowed":
      return;
    case "PacketDelivered":
      writer.f64(outcome.data.rttMillis);
      writer.u8(encodeEvidence(outcome.data.evidence));
      writer.optionalFixed(outcome.data.packetHash, PACKET_HASH_LENGTH, "packet hash");
      return;
    case "InterfaceAttached":
    case "InterfaceDetached":
      writer.fixed(outcome.data.interface, INTERFACE_ID_LENGTH, "interface id");
      return;
    case "LinkEstablished":
      writer.fixed(outcome.data.linkId, LINK_ID_LENGTH, "link id");
      writer.f64(outcome.data.rttMillis);
      return;
    case "PathDiscovered":
      writer.f64(outcome.data.hops);
      return;
    case "ResponseReceived":
      writer.bytes(outcome.data.data);
      writer.f64(outcome.data.rttMillis);
      return;
    case "ResponseSent":
      writer.f64(outcome.data.rttMillis);
      return;
  }
}

function decodeOutcome(reader: WireReader): CommandOutcome {
  const tag = reader.code(OUTCOME_TAGS, "command outcome");
  switch (tag) {
    case "Announced":
      return Tag("Announced");
    case "PacketDelivered": {
      const rttMillis = reader.f64();
      const evidence = decodeEvidence(reader.u8());
      const packetHash = reader.optionalFixed(PACKET_HASH_LENGTH) as PacketHash | undefined;
      return Tag("PacketDelivered", packetHash === undefined
        ? { rttMillis, evidence }
        : { rttMillis, evidence, packetHash });
    }
    case "LinkCloseQueued":
      return Tag("LinkCloseQueued");
    case "InterfaceAttached":
      return Tag("InterfaceAttached", {
        interface: reader.fixed(INTERFACE_ID_LENGTH) as InterfaceId,
      });
    case "InterfaceDetached":
      return Tag("InterfaceDetached", {
        interface: reader.fixed(INTERFACE_ID_LENGTH) as InterfaceId,
      });
    case "LinkEstablished":
      return Tag("LinkEstablished", {
        linkId: reader.fixed(LINK_ID_LENGTH) as LinkId,
        rttMillis: reader.f64(),
      });
    case "PathDiscovered":
      return Tag("PathDiscovered", { hops: reader.f64() });
    case "Identified":
      return Tag("Identified");
    case "ResponseReceived":
      return Tag("ResponseReceived", { data: reader.bytes(), rttMillis: reader.f64() });
    case "ResponseSent":
      return Tag("ResponseSent", { rttMillis: reader.f64() });
    case "ResourceSent":
      return Tag("ResourceSent");
    case "ResourceStrategySet":
      return Tag("ResourceStrategySet");
    case "RequesterAllowed":
      return Tag("RequesterAllowed");
  }
}

function encodedFailureSize(failure: CommandFailure): number {
  const format = FAILURE_FORMATS[failure.tag];
  return 1 + (format.data === "Detail"
    ? stringSize((failure.data as { readonly detail: string }).detail)
    : 0);
}

function encodeFailure(failure: CommandFailure, writer: WireWriter): void {
  const format = FAILURE_FORMATS[failure.tag];
  writer.u8(format.code);
  if (format.data === "Detail") {
    const detail = (failure.data as { readonly detail?: unknown }).detail;
    if (typeof detail !== "string") {
      throw new TypeError("command failure detail is not a string");
    }
    writer.string(detail);
  }
}

function decodeFailure(reader: WireReader): CommandFailure {
  const tag = reader.code(FAILURE_TAGS, "command failure");
  const format = FAILURE_FORMATS[tag];
  return format.data === "Detail"
    ? Tag(tag, { detail: reader.string() }) as CommandFailure
    : Tag(tag) as CommandFailure;
}

function responseTimeoutSize(value: ResponseTimeout): number {
  return value.tag === "LinkDefault" ? 1 : 9;
}

function encodeResponseTimeout(value: ResponseTimeout, writer: WireWriter): void {
  if (value.tag === "LinkDefault") {
    writer.u8(0);
    return;
  }
  if (value.tag === "Exact") {
    writer.u8(1);
    writer.f64(value.data.millis);
    return;
  }
  throw new TypeError("worker command contains an unknown response timeout");
}

function decodeResponseTimeout(reader: WireReader): ResponseTimeout {
  const code = reader.u8();
  if (code === 0) {
    return Tag("LinkDefault");
  }
  if (code === 1) {
    return Tag("Exact", { millis: reader.f64() });
  }
  throw new TypeError("worker command contains an unknown response timeout");
}

function resourceStrategySize(value: ResourceStrategy): number {
  return value.tag === "Refuse" ? 1 : 10;
}

function encodeResourceStrategy(value: ResourceStrategy, writer: WireWriter): void {
  if (value.tag === "Refuse") {
    writer.u8(0);
    return;
  }
  if (value.tag === "Accept") {
    writer.u8(1);
    writer.f64(value.data.maximumUncompressedBytes);
    writer.u8(value.data.acceptCompressed ? 1 : 0);
    return;
  }
  throw new TypeError("worker command contains an unknown resource strategy");
}

function decodeResourceStrategy(reader: WireReader): ResourceStrategy {
  const code = reader.u8();
  if (code === 0) {
    return Tag("Refuse");
  }
  if (code === 1) {
    const maximumUncompressedBytes = reader.f64();
    const acceptCompressed = reader.boolean();
    return Tag("Accept", { maximumUncompressedBytes, acceptCompressed });
  }
  throw new TypeError("worker command contains an unknown resource strategy");
}

function encodeResourceCompression(
  value: ResourceCompression,
  writer: WireWriter,
): void {
  if (value.tag === "Auto") {
    writer.u8(0);
    return;
  }
  if (value.tag === "Never") {
    writer.u8(1);
    return;
  }
  throw new TypeError("worker command contains an unknown resource compression");
}

function decodeResourceCompression(reader: WireReader): ResourceCompression {
  const code = reader.u8();
  if (code === 0) {
    return Tag("Auto");
  }
  if (code === 1) {
    return Tag("Never");
  }
  throw new TypeError("worker command contains an unknown resource compression");
}

function encodeEvidence(value: DeliveryEvidenceKind): number {
  if (value === "ExplicitProof") {
    return 0;
  }
  if (value === "ImplicitProof") {
    return 1;
  }
  if (value === "Response") {
    return 2;
  }
  throw new TypeError("command outcome contains an unknown delivery evidence kind");
}

function decodeEvidence(code: number): DeliveryEvidenceKind {
  if (code === 0) {
    return "ExplicitProof";
  }
  if (code === 1) {
    return "ImplicitProof";
  }
  if (code === 2) {
    return "Response";
  }
  throw new TypeError("command outcome contains an unknown delivery evidence kind");
}

function bytesSize(value: Uint8Array): number {
  if (!(value instanceof Uint8Array) || value.byteLength > 0xffff_ffff) {
    throw new TypeError("worker codec byte field is invalid");
  }
  return 4 + value.byteLength;
}

function stringSize(value: string): number {
  return 4 + textEncoder.encode(value).byteLength;
}

function addSize(left: number, right: number): number {
  const sum = left + right;
  if (!Number.isSafeInteger(sum) || sum > 0xffff_ffff) {
    throw new TypeError("worker codec frame exceeds its byte bound");
  }
  return sum;
}

function requireItemCount(count: number): void {
  if (!Number.isSafeInteger(count) || count > MAXIMUM_WIRE_BATCH_ITEMS) {
    throw new TypeError("worker codec frame exceeds its item bound");
  }
}

function invertCodes<Names extends string>(
  codes: Readonly<Record<Names, number>>,
  label: string,
): readonly Names[] {
  const names: (Names | undefined)[] = [];
  for (const name of Object.keys(codes) as Names[]) {
    const code = codes[name];
    if (!Number.isSafeInteger(code) || code < 0 || code > 0xff || names[code] !== undefined) {
      throw new TypeError(`${label} codes are invalid`);
    }
    names[code] = name;
  }
  for (let code = 0; code < names.length; code += 1) {
    if (names[code] === undefined) {
      throw new TypeError(`${label} codes are not contiguous`);
    }
  }
  return names as Names[];
}

function invertFormats<Names extends string>(
  formats: Readonly<Record<Names, { readonly code: number }>>,
  label: string,
): readonly Names[] {
  const codes = Object.fromEntries(
    (Object.keys(formats) as Names[]).map((name) => [name, formats[name].code]),
  ) as Record<Names, number>;
  return invertCodes(codes, label);
}

class WireWriter {
  readonly #buffer: ArrayBuffer;
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(byteLength: number) {
    if (!Number.isSafeInteger(byteLength) || byteLength < 0 || byteLength > 0xffff_ffff) {
      throw new TypeError("worker codec frame has an invalid byte length");
    }
    this.#buffer = new ArrayBuffer(byteLength);
    this.#bytes = new Uint8Array(this.#buffer);
    this.#view = new DataView(this.#buffer);
  }

  u8(value: number): void {
    if (!Number.isSafeInteger(value) || value < 0 || value > 0xff) {
      throw new TypeError("worker codec byte value is invalid");
    }
    this.#view.setUint8(this.#offset, value);
    this.#offset += 1;
  }

  u32(value: number): void {
    if (!Number.isSafeInteger(value) || value < 0 || value > 0xffff_ffff) {
      throw new TypeError("worker codec u32 value is invalid");
    }
    this.#view.setUint32(this.#offset, value, true);
    this.#offset += 4;
  }

  f64(value: number): void {
    if (typeof value !== "number") {
      throw new TypeError("worker codec number value is invalid");
    }
    this.#view.setFloat64(this.#offset, value, true);
    this.#offset += 8;
  }

  fixed(value: Uint8Array, length: number, label: string): void {
    if (!(value instanceof Uint8Array) || value.byteLength !== length) {
      throw new TypeError(`${label} has the wrong byte length`);
    }
    this.#rawBytes(value);
  }

  optionalFixed(
    value: Uint8Array | undefined,
    length: number,
    label: string,
  ): void {
    this.u8(value === undefined ? 0 : 1);
    if (value !== undefined) {
      this.fixed(value, length, label);
    }
  }

  bytes(value: Uint8Array): void {
    this.u32(bytesSize(value) - 4);
    this.#rawBytes(value);
  }

  optionalBytes(value: Uint8Array | undefined): void {
    this.u8(value === undefined ? 0 : 1);
    if (value !== undefined) {
      this.bytes(value);
    }
  }

  optionalNumber(value: number | undefined): void {
    this.u8(value === undefined ? 0 : 1);
    if (value !== undefined) {
      this.f64(value);
    }
  }

  string(value: string): void {
    const bytes = textEncoder.encode(value);
    this.u32(bytes.byteLength);
    this.#rawBytes(bytes);
  }

  finish(): ArrayBuffer {
    if (this.#offset !== this.#buffer.byteLength) {
      throw new TypeError("worker codec did not fill its frame");
    }
    return this.#buffer;
  }

  #rawBytes(value: Uint8Array): void {
    if (value.byteLength > this.#bytes.byteLength - this.#offset) {
      throw new TypeError("worker codec write exceeds its frame");
    }
    this.#bytes.set(value, this.#offset);
    this.#offset += value.byteLength;
  }
}

class WireReader {
  readonly #buffer: ArrayBuffer;
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(buffer: ArrayBuffer) {
    if (!(buffer instanceof ArrayBuffer)) {
      throw new TypeError("worker codec payload is not an ArrayBuffer");
    }
    this.#buffer = buffer;
    this.#bytes = new Uint8Array(buffer);
    this.#view = new DataView(buffer);
  }

  magic(expected: number): void {
    if (this.u32() !== expected) {
      throw new TypeError("worker codec frame has an unknown format");
    }
  }

  count(): number {
    const count = this.u32();
    if (count > MAXIMUM_WIRE_BATCH_ITEMS) {
      throw new TypeError("worker codec frame exceeds its item bound");
    }
    return count;
  }

  safeId(): number {
    const value = this.f64();
    if (!Number.isSafeInteger(value) || value <= 0) {
      throw new TypeError("worker codec frame contains an invalid call id");
    }
    return value;
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

  f64(): number {
    this.#require(8);
    const value = this.#view.getFloat64(this.#offset, true);
    this.#offset += 8;
    return value;
  }

  boolean(): boolean {
    const value = this.u8();
    if (value !== 0 && value !== 1) {
      throw new TypeError("worker codec frame contains an invalid boolean");
    }
    return value === 1;
  }

  fixed(length: number): Uint8Array {
    return this.#copy(length);
  }

  optionalFixed(length: number): Uint8Array | undefined {
    const present = this.boolean();
    return present ? this.fixed(length) : undefined;
  }

  bytes(): Uint8Array {
    return this.#copy(this.u32());
  }

  optionalBytes(): Uint8Array | undefined {
    return this.boolean() ? this.bytes() : undefined;
  }

  optionalNumber(): number | undefined {
    return this.boolean() ? this.f64() : undefined;
  }

  string(): string {
    return textDecoder.decode(this.#copy(this.u32()));
  }

  code<Names extends string>(names: readonly Names[], label: string): Names {
    const name = names[this.u8()];
    if (name === undefined) {
      throw new TypeError(`${label} code is unknown`);
    }
    return name;
  }

  requireFinished(): void {
    if (this.#offset !== this.#buffer.byteLength) {
      throw new TypeError("worker codec frame contains trailing bytes");
    }
  }

  #copy(length: number): Uint8Array {
    this.#require(length);
    const value = this.#bytes.slice(this.#offset, this.#offset + length);
    this.#offset += length;
    return value;
  }

  #require(length: number): void {
    if (
      !Number.isSafeInteger(length) ||
      length < 0 ||
      length > this.#buffer.byteLength - this.#offset
    ) {
      throw new TypeError("worker codec read exceeds its frame");
    }
  }
}

import type {
  PacketFrame,
  PrnsRuntimeBinding,
  WebSocketDecodeBatchBinding,
  WebSocketFramingCodecBinding,
} from "../../prns-js/src/browser/index.js";

const FRAME_CAP = 572;
const KISS_FLAG = 0xc0;
const KISS_ESCAPE = 0xdb;
const HDLC_FLAG = 0x7e;
const HDLC_ESCAPE = 0x7d;

type MockWebSocketWireFraming = "raw" | "hdlc" | "kiss";

export class MockWebSocketFramingCodec
  implements WebSocketFramingCodecBinding
{
  #framing: MockWebSocketWireFraming | undefined;
  #pending: Uint8Array | undefined;
  #inbound: Uint8Array<ArrayBufferLike> = new Uint8Array();

  constructor(selection: string) {
    switch (selection) {
      case "auto":
        this.#framing = undefined;
        break;
      case "raw":
      case "hdlc":
      case "kiss":
        this.#framing = selection;
        break;
      default:
        throw new Error(`unknown WebSocket framing selection ${selection}`);
    }
  }

  messageCap(): number {
    return this.#framing === "raw" ? FRAME_CAP : FRAME_CAP * 2 + 3;
  }

  canReadOutbound(): boolean {
    return this.#pending === undefined;
  }

  rawFallbackIsArmed(): boolean {
    return this.#framing === undefined && this.#pending !== undefined;
  }

  isDetecting(): boolean {
    return this.#framing === undefined;
  }

  rawFallbackDelayMillis(): number {
    return 250;
  }

  decode(message: Uint8Array): WebSocketDecodeBatchBinding {
    let resolvedOutbound: Uint8Array | undefined;
    if (this.#framing === undefined) {
      const first = message[0];
      resolvedOutbound = this.#resolve(
        first === KISS_FLAG ? "kiss" : first === HDLC_FLAG ? "hdlc" : "raw",
      );
    }
    if (this.#framing === "raw") {
      return mockWebSocketDecodeBatch(
        message.length === 0 ? [] : [message.slice()],
        resolvedOutbound,
      );
    }
    this.#inbound = joinedBytes(this.#inbound, message);
    const packets: Uint8Array[] = [];
    const flag = this.#framing === "kiss" ? KISS_FLAG : HDLC_FLAG;
    while (true) {
      const start = this.#inbound.indexOf(flag);
      if (start < 0) {
        this.#inbound = new Uint8Array();
        return mockWebSocketDecodeBatch(packets, resolvedOutbound);
      }
      const end = this.#inbound.indexOf(flag, start + 1);
      if (end < 0) {
        this.#inbound = this.#inbound.slice(start);
        return mockWebSocketDecodeBatch(packets, resolvedOutbound);
      }
      const framed = this.#inbound.slice(start + 1, end);
      this.#inbound = this.#inbound.slice(end);
      const packet =
        this.#framing === "kiss"
          ? unescapeBytes(framed.slice(1), KISS_ESCAPE, 0xdc, 0xdd)
          : unescapeBytes(framed, HDLC_ESCAPE, 0x5e, 0x5d);
      if (packet.length > 0) {
        packets.push(packet);
      }
    }
  }

  stageOutbound(packet: PacketFrame): Uint8Array | undefined {
    if (this.#framing === undefined) {
      if (this.#pending !== undefined) {
        throw new Error("WebSocket framing is awaiting evidence");
      }
      this.#pending = packet.slice();
      return undefined;
    }
    return encodeMockWebSocketPacket(this.#framing, packet);
  }

  resolveRawFallback(): Uint8Array | undefined {
    return this.#resolve("raw");
  }

  #resolve(framing: MockWebSocketWireFraming): Uint8Array | undefined {
    this.#framing = framing;
    if (this.#pending === undefined) {
      return undefined;
    }
    const outbound = encodeMockWebSocketPacket(framing, this.#pending);
    this.#pending = undefined;
    return outbound;
  }
}

function mockWebSocketDecodeBatch(
  packets: readonly Uint8Array[],
  resolvedOutbound: Uint8Array | undefined,
): WebSocketDecodeBatchBinding {
  return resolvedOutbound === undefined
    ? { packets }
    : { packets, resolvedOutbound };
}

function encodeMockWebSocketPacket(
  framing: MockWebSocketWireFraming,
  packet: Uint8Array,
): Uint8Array {
  if (framing === "raw") {
    return packet.slice();
  }
  const flag = framing === "kiss" ? KISS_FLAG : HDLC_FLAG;
  const escape = framing === "kiss" ? KISS_ESCAPE : HDLC_ESCAPE;
  const escapedFlag = framing === "kiss" ? 0xdc : 0x5e;
  const escapedEscape = framing === "kiss" ? 0xdd : 0x5d;
  const encoded = [flag];
  if (framing === "kiss") {
    encoded.push(0);
  }
  for (const byte of packet) {
    if (byte === flag) {
      encoded.push(escape, escapedFlag);
    } else if (byte === escape) {
      encoded.push(escape, escapedEscape);
    } else {
      encoded.push(byte);
    }
  }
  encoded.push(flag);
  return new Uint8Array(encoded);
}

function joinedBytes(left: Uint8Array, right: Uint8Array): Uint8Array {
  const joined = new Uint8Array(left.length + right.length);
  joined.set(left);
  joined.set(right, left.length);
  return joined;
}

function unescapeBytes(
  framed: Uint8Array,
  escape: number,
  escapedFlag: number,
  escapedEscape: number,
): Uint8Array {
  const decoded: number[] = [];
  for (let index = 0; index < framed.length; index += 1) {
    const byte = framed[index];
    if (byte !== escape) {
      decoded.push(byte ?? 0);
      continue;
    }
    index += 1;
    const escaped = framed[index];
    if (escaped === escapedFlag) {
      decoded.push(escape === KISS_ESCAPE ? KISS_FLAG : HDLC_FLAG);
    } else if (escaped === escapedEscape) {
      decoded.push(escape);
    }
  }
  return new Uint8Array(decoded);
}

export class MockRuntimeBase implements PrnsRuntimeBinding {
  registerInterface(
    _options: Parameters<PrnsRuntimeBinding["registerInterface"]>[0],
  ): ReturnType<PrnsRuntimeBinding["registerInterface"]> {
    return unexpectedRuntimeCall("registerInterface");
  }

  removeInterface(
    _options: Parameters<PrnsRuntimeBinding["removeInterface"]>[0],
  ): ReturnType<PrnsRuntimeBinding["removeInterface"]> {
    return unexpectedRuntimeCall("removeInterface");
  }

  bluetoothIdentity(): ReturnType<
    PrnsRuntimeBinding["bluetoothIdentity"]
  > {
    return unexpectedRuntimeCall("bluetoothIdentity");
  }

  registerSingleDestination(
    _options: Parameters<
      PrnsRuntimeBinding["registerSingleDestination"]
    >[0],
  ): ReturnType<PrnsRuntimeBinding["registerSingleDestination"]> {
    return unexpectedRuntimeCall("registerSingleDestination");
  }

  registerNodePage(
    _options: Parameters<PrnsRuntimeBinding["registerNodePage"]>[0],
  ): ReturnType<PrnsRuntimeBinding["registerNodePage"]> {
    return unexpectedRuntimeCall("registerNodePage");
  }

  announce(
    _options: Parameters<PrnsRuntimeBinding["announce"]>[0],
  ): ReturnType<PrnsRuntimeBinding["announce"]> {
    return unexpectedRuntimeCall("announce");
  }

  sendSinglePacket(
    _options: Parameters<PrnsRuntimeBinding["sendSinglePacket"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendSinglePacket"]> {
    return unexpectedRuntimeCall("sendSinglePacket");
  }

  establishLink(
    _options: Parameters<PrnsRuntimeBinding["establishLink"]>[0],
  ): ReturnType<PrnsRuntimeBinding["establishLink"]> {
    return unexpectedRuntimeCall("establishLink");
  }

  requestPath(
    _options: Parameters<PrnsRuntimeBinding["requestPath"]>[0],
  ): ReturnType<PrnsRuntimeBinding["requestPath"]> {
    return unexpectedRuntimeCall("requestPath");
  }

  identify(
    _options: Parameters<PrnsRuntimeBinding["identify"]>[0],
  ): ReturnType<PrnsRuntimeBinding["identify"]> {
    return unexpectedRuntimeCall("identify");
  }

  sendLinkPacket(
    _options: Parameters<PrnsRuntimeBinding["sendLinkPacket"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendLinkPacket"]> {
    return unexpectedRuntimeCall("sendLinkPacket");
  }

  request(
    _options: Parameters<PrnsRuntimeBinding["request"]>[0],
  ): ReturnType<PrnsRuntimeBinding["request"]> {
    return unexpectedRuntimeCall("request");
  }

  respond(
    _options: Parameters<PrnsRuntimeBinding["respond"]>[0],
  ): ReturnType<PrnsRuntimeBinding["respond"]> {
    return unexpectedRuntimeCall("respond");
  }

  resourceSegmentPlan(
    _options: Parameters<PrnsRuntimeBinding["resourceSegmentPlan"]>[0],
  ): ReturnType<PrnsRuntimeBinding["resourceSegmentPlan"]> {
    return unexpectedRuntimeCall("resourceSegmentPlan");
  }

  sendResourceSegment(
    _options: Parameters<PrnsRuntimeBinding["sendResourceSegment"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendResourceSegment"]> {
    return unexpectedRuntimeCall("sendResourceSegment");
  }

  setLinkResourceStrategy(
    _options: Parameters<
      PrnsRuntimeBinding["setLinkResourceStrategy"]
    >[0],
  ): ReturnType<PrnsRuntimeBinding["setLinkResourceStrategy"]> {
    return unexpectedRuntimeCall("setLinkResourceStrategy");
  }

  setDestinationResourceStrategy(
    _options: Parameters<
      PrnsRuntimeBinding["setDestinationResourceStrategy"]
    >[0],
  ): ReturnType<PrnsRuntimeBinding["setDestinationResourceStrategy"]> {
    return unexpectedRuntimeCall("setDestinationResourceStrategy");
  }

  sendChannelMessage(
    _options: Parameters<PrnsRuntimeBinding["sendChannelMessage"]>[0],
  ): ReturnType<PrnsRuntimeBinding["sendChannelMessage"]> {
    return unexpectedRuntimeCall("sendChannelMessage");
  }

  allowRequester(
    _options: Parameters<PrnsRuntimeBinding["allowRequester"]>[0],
  ): ReturnType<PrnsRuntimeBinding["allowRequester"]> {
    return unexpectedRuntimeCall("allowRequester");
  }

  closeLink(
    _options: Parameters<PrnsRuntimeBinding["closeLink"]>[0],
  ): ReturnType<PrnsRuntimeBinding["closeLink"]> {
    return unexpectedRuntimeCall("closeLink");
  }

  ingest(
    _options: Parameters<PrnsRuntimeBinding["ingest"]>[0],
  ): ReturnType<PrnsRuntimeBinding["ingest"]> {
    return unexpectedRuntimeCall("ingest");
  }

  drainEvents(): ReturnType<PrnsRuntimeBinding["drainEvents"]> {
    return unexpectedRuntimeCall("drainEvents");
  }

  drainOutbound(): ReturnType<PrnsRuntimeBinding["drainOutbound"]> {
    return unexpectedRuntimeCall("drainOutbound");
  }

  persistedState(
    _options: Parameters<PrnsRuntimeBinding["persistedState"]>[0],
  ): ReturnType<PrnsRuntimeBinding["persistedState"]> {
    return unexpectedRuntimeCall("persistedState");
  }

  restorePersistedState(
    _options: Parameters<PrnsRuntimeBinding["restorePersistedState"]>[0],
  ): ReturnType<PrnsRuntimeBinding["restorePersistedState"]> {
    return unexpectedRuntimeCall("restorePersistedState");
  }

  snapshot(): ReturnType<PrnsRuntimeBinding["snapshot"]> {
    return unexpectedRuntimeCall("snapshot");
  }
}

function unexpectedRuntimeCall(operation: string): never {
  throw new Error(`unexpected mock runtime call: ${operation}`);
}

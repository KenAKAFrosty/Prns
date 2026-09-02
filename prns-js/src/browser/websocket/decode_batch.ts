import type { WebSocketDecodeBatchBinding } from "../runtime_contract.js";

const WEBSOCKET_DECODE_BATCH_MAGIC = 0x4453_5750;
const WEBSOCKET_DECODE_BATCH_VERSION = 1;
const WEBSOCKET_DECODE_BATCH_RESOLVED_OUTBOUND = 1;
const WEBSOCKET_DECODE_BATCH_MINIMUM_PACKET_BYTES = 4;

export function parseWebSocketDecodeBatch(
  bytes: Uint8Array,
): WebSocketDecodeBatchBinding {
  const reader = new WebSocketDecodeBatchReader(bytes);
  if (reader.u32() !== WEBSOCKET_DECODE_BATCH_MAGIC) {
    throw new TypeError("WebSocket decode batch magic is invalid");
  }
  if (reader.u16() !== WEBSOCKET_DECODE_BATCH_VERSION) {
    throw new TypeError("WebSocket decode batch version is unsupported");
  }
  const flags = reader.u16();
  if ((flags & ~WEBSOCKET_DECODE_BATCH_RESOLVED_OUTBOUND) !== 0) {
    throw new TypeError("WebSocket decode batch flags are unsupported");
  }
  const count = reader.u32();
  if (
    count >
      Math.floor(reader.remaining / WEBSOCKET_DECODE_BATCH_MINIMUM_PACKET_BYTES)
  ) {
    throw new TypeError("WebSocket decode batch packet count exceeds its bytes");
  }
  const packets = new Array<Uint8Array>(count);
  for (let index = 0; index < count; index += 1) {
    const packet = reader.bytes(reader.u32());
    if (packet.byteLength === 0) {
      throw new TypeError("WebSocket decode batch contains an empty packet");
    }
    packets[index] = packet;
  }
  const resolvedOutbound =
    (flags & WEBSOCKET_DECODE_BATCH_RESOLVED_OUTBOUND) === 0
      ? undefined
      : reader.bytes(reader.u32());
  if (resolvedOutbound?.byteLength === 0) {
    throw new TypeError("WebSocket decode batch contains an empty resolved frame");
  }
  reader.requireFinished();
  return resolvedOutbound === undefined
    ? { packets }
    : { packets, resolvedOutbound };
}

class WebSocketDecodeBatchReader {
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(bytes: Uint8Array) {
    if (!(bytes instanceof Uint8Array)) {
      throw new TypeError("WebSocket decode batch is not a Uint8Array");
    }
    this.#bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  get remaining(): number {
    return this.#bytes.byteLength - this.#offset;
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
      throw new TypeError("WebSocket decode batch contains trailing bytes");
    }
  }

  #require(length: number): void {
    if (!Number.isSafeInteger(length) || length < 0 || length > this.remaining) {
      throw new TypeError("WebSocket decode batch is truncated");
    }
  }
}

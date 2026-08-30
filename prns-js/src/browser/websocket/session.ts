import { Tag } from "../../casework.js";
import type {
  InterfaceId,
  WebSocketFramingSelection,
} from "../../contract.js";
import { describeHostError } from "../host_errors.js";
import {
  closeFailed,
  closedSessionOutcome,
  delay,
  describeInterfaceSessionFailure,
  hasCleanupFailures,
  unexpectedSessionFailure,
} from "../session.js";
import type {
  InterfaceCleanupFailure,
  InterfaceCloseOutcome,
  InterfaceSessionFailure,
  InterfaceSessionStatus,
} from "../interface_contract.js";
import type { WebSocketFramingCodecBinding } from "../runtime_contract.js";
import type { PacketFrame } from "../values.js";
import type {
  WebSocketRuntimeHost,
  WebSocketSession,
} from "./index.js";

type SessionWriteOutcome = Tag<"Written"> | InterfaceSessionFailure;
type SessionHandleOutcome = Tag<"Handled"> | InterfaceSessionFailure;
type WebSocketDecodeOutcome =
  | Tag<"Decoded", Uint8Array>
  | Extract<
      InterfaceSessionFailure,
      | Tag<"UnsupportedFrame", unknown>
      | Tag<"FrameTooLarge", unknown>
      | Tag<"TransferFailed", unknown>
    >;
type FramingWaitOutcome = Tag<"FramingReady"> | Tag<"FallbackDue">;

const BUFFER_POLL_MS = 4;
const MIN_BUFFER_LIMIT = 1024 * 1024;
const WEBSOCKET_CONNECTING = 0;
const WEBSOCKET_OPEN = 1;

export class BrowserWebSocketSession implements WebSocketSession {
  readonly name = "websocket" as const;
  readonly interfaceId: InterfaceId;
  readonly url: string;
  readonly framing: WebSocketFramingSelection;

  readonly #host: WebSocketRuntimeHost;
  readonly #socket: WebSocket;
  readonly #frameCap: number;
  readonly #codec: WebSocketFramingCodecBinding;
  readonly #bufferLimit: number;
  readonly #release: () => void;
  readonly #framingWaiters = new Set<() => void>();
  readonly #statusListeners = new Set<(status: InterfaceSessionStatus) => void>();
  #readQueue: Promise<void> = Promise.resolve();
  #writeQueue: Promise<SessionWriteOutcome> = Promise.resolve(Tag("Written"));
  #closed = false;
  #released = false;
  #status: InterfaceSessionStatus = Tag("Active");
  #closePromise: Promise<InterfaceCloseOutcome> | undefined;

  constructor(
    host: WebSocketRuntimeHost,
    socket: WebSocket,
    interfaceId: InterfaceId,
    url: string,
    frameCap: number,
    framing: WebSocketFramingSelection,
    codec: WebSocketFramingCodecBinding,
    release: () => void,
  ) {
    this.#host = host;
    this.#socket = socket;
    this.interfaceId = interfaceId;
    this.url = url;
    this.#frameCap = frameCap;
    this.framing = framing;
    this.#codec = codec;
    this.#bufferLimit = Math.max(MIN_BUFFER_LIMIT, codec.messageCap() * 2);
    this.#release = release;
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  subscribeStatus(
    changed: (status: InterfaceSessionStatus) => void,
  ): () => void {
    this.#statusListeners.add(changed);
    let subscribed = true;
    return () => {
      if (!subscribed) {
        return;
      }
      subscribed = false;
      this.#statusListeners.delete(changed);
    };
  }

  start(): void {
    this.#socket.addEventListener("message", (event) => {
      this.#enqueueMessage(event);
    });
    this.#socket.addEventListener("close", () => {
      this.#handleClose();
    });
    this.#socket.addEventListener("error", () => {
      void this.#fail(
        Tag("Disconnected", {
          detail: `WebSocket connection failed for ${this.url}`,
        }),
      );
    });
    void this.#outboundLoop();
  }

  close(): Promise<InterfaceCloseOutcome> {
    if (this.#closePromise !== undefined) {
      return this.#closePromise;
    }
    if (this.#closed) {
      return Promise.resolve(closedSessionOutcome(this.#status));
    }
    this.#closePromise = this.#performClose().finally(() => {
      this.#closePromise = undefined;
    });
    return this.#closePromise;
  }

  async #performClose(): Promise<InterfaceCloseOutcome> {
    this.#closed = true;
    this.#resolveFramingWaiters();
    const causes: InterfaceCleanupFailure[] = [];
    const detached = this.#host.deactivateInterface(this.interfaceId);
    if (detached.tag !== "Detached") {
      causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
    }
    this.#releaseOnce();
    const socketFailure = closeBrowserWebSocket(this.#socket);
    if (socketFailure) {
      causes.push(socketFailure);
    }
    const pendingWrite = await this.#writeQueue;
    if (pendingWrite.tag !== "Written") {
      causes.push(
        Tag("TransportCloseFailed", {
          detail: describeInterfaceSessionFailure(pendingWrite),
        }),
      );
    }
    if (hasCleanupFailures(causes)) {
      const failed = closeFailed(causes);
      this.#replaceStatus(Tag("Failed", failed));
      return failed;
    }
    this.#replaceStatus(Tag("Closed"));
    return Tag("Closed");
  }

  #enqueueMessage(event: MessageEvent): void {
    this.#readQueue = this.#readQueue
      .then(async () => {
        const handled = await this.#handleMessage(event);
        if (handled.tag !== "Handled" && !this.#closed) {
          await this.#fail(handled);
        }
      })
      .catch(async (error: unknown) => {
        if (!this.#closed) {
          await this.#fail(unexpectedSessionFailure(error));
        }
      });
  }

  async #handleMessage(event: MessageEvent): Promise<SessionHandleOutcome> {
    const decoded = await websocketMessageBytes(
      event.data,
      this.#codec.messageCap(),
    );
    if (decoded.tag !== "Decoded") {
      return decoded;
    }
    const batch = this.#codec.decode(decoded.data);
    if (this.#codec.canReadOutbound()) {
      this.#resolveFramingWaiters();
    }
    for (const packet of batch.packets) {
      if (this.#closed) {
        return Tag("Handled");
      }
      const ingested = this.#host.webSocketIngest(this.interfaceId, packet);
      if (ingested.tag !== "Accepted") {
        return ingested;
      }
    }
    const pending = batch.resolvedOutbound;
    if (pending !== undefined) {
      const written = await this.#writeEncodedFrame(pending);
      if (written.tag !== "Written") {
        return written;
      }
    }
    return Tag("Handled");
  }

  #handleClose(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    this.#resolveFramingWaiters();
    const detached = this.#host.deactivateInterface(this.interfaceId);
    this.#replaceStatus(
      detached.tag === "Detached" ? Tag("Closed") : Tag("Failed", detached),
    );
    this.#releaseOnce();
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        if (this.#codec.rawFallbackIsArmed()) {
          const wait = await this.#waitForRawFallback();
          if (this.#closed) {
            return;
          }
          if (
            wait.tag === "FallbackDue" &&
            this.#codec.rawFallbackIsArmed()
          ) {
            const pending = this.#codec.releaseRawFallback();
            if (pending !== undefined) {
              const written = await this.#writeEncodedFrame(pending);
              if (written.tag !== "Written") {
                await this.#fail(written);
                return;
              }
            }
          }
          continue;
        }
        if (!this.#codec.canReadOutbound()) {
          await this.#waitForFramingReadiness();
          continue;
        }
        const maximumFrames = this.#codec.canStageMultipleOutbound()
          ? Number.MAX_SAFE_INTEGER
          : 1;
        const outbound = await this.#host.nextOutboundFor(
          this.interfaceId,
          maximumFrames,
        );
        if (outbound.tag === "InterfaceDetached") {
          return;
        }
        if (outbound.tag !== "Outbound") {
          await this.#fail(outbound);
          return;
        }
        for (const frame of outbound.data) {
          const written = await this.#writeFrame(frame.bytes);
          if (written.tag !== "Written") {
            await this.#fail(written);
            return;
          }
        }
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #fail(sessionFailure: InterfaceSessionFailure): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#replaceStatus(Tag("Failed", sessionFailure));
    this.#closed = true;
    this.#resolveFramingWaiters();
    this.#host.deactivateInterface(this.interfaceId);
    this.#releaseOnce();
    await this.#writeQueue;
    closeBrowserWebSocket(this.#socket);
  }

  #waitForFramingReadiness(): Promise<void> {
    if (this.#closed || this.#codec.canReadOutbound()) {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      this.#framingWaiters.add(resolve);
      if (this.#closed || this.#codec.canReadOutbound()) {
        this.#framingWaiters.delete(resolve);
        resolve();
      }
    });
  }

  #waitForRawFallback(): Promise<FramingWaitOutcome> {
    if (this.#closed || !this.#codec.rawFallbackIsArmed()) {
      return Promise.resolve(Tag("FramingReady"));
    }
    return new Promise((resolve) => {
      let settled = false;
      const framingReady = (): void => {
        settle(Tag("FramingReady"));
      };
      const timer = globalThis.setTimeout(() => {
        settle(Tag("FallbackDue"));
      }, this.#codec.rawFallbackDelayMillis());
      const settle = (outcome: FramingWaitOutcome): void => {
        if (settled) {
          return;
        }
        settled = true;
        globalThis.clearTimeout(timer);
        this.#framingWaiters.delete(framingReady);
        resolve(outcome);
      };
      this.#framingWaiters.add(framingReady);
      if (this.#closed || !this.#codec.rawFallbackIsArmed()) {
        framingReady();
      }
    });
  }

  #resolveFramingWaiters(): void {
    const waiters = [...this.#framingWaiters];
    this.#framingWaiters.clear();
    for (const resolve of waiters) {
      resolve();
    }
  }

  #replaceStatus(status: InterfaceSessionStatus): void {
    this.#status = status;
    for (const changed of this.#statusListeners) {
      changed(status);
    }
  }

  async #writeFrame(frame: PacketFrame): Promise<SessionWriteOutcome> {
    if (this.#closed || frame.length === 0) {
      return Tag("Written");
    }
    if (frame.length > this.#frameCap) {
      return Tag("FrameTooLarge", {
        length: frame.length,
        maximum: this.#frameCap,
      });
    }
    let encoded: Uint8Array | undefined;
    try {
      encoded = this.#codec.stageOutbound(frame);
    } catch (error) {
      return Tag("TransferFailed", {
        direction: "Outbound",
        detail: describeHostError(error),
      });
    }
    if (encoded === undefined) {
      return Tag("Written");
    }
    return this.#writeEncodedFrame(encoded);
  }

  async #writeEncodedFrame(frame: Uint8Array): Promise<SessionWriteOutcome> {
    if (this.#closed || frame.length === 0) {
      return Tag("Written");
    }
    const write = this.#writeQueue
      .then(async (previous): Promise<SessionWriteOutcome> => {
        if (previous.tag !== "Written" || this.#closed) {
          return previous;
        }
        while (!this.#closed && this.#socket.bufferedAmount > this.#bufferLimit) {
          await delay(BUFFER_POLL_MS);
        }
        if (this.#closed) {
          return Tag("Written");
        }
        if (this.#socket.readyState !== WEBSOCKET_OPEN) {
          return Tag("Disconnected", {
            detail: `WebSocket is not open for ${this.url}`,
          });
        }
        try {
          this.#socket.send(frame);
          return Tag("Written");
        } catch (error) {
          return Tag("Disconnected", { detail: describeHostError(error) });
        }
      })
      .catch((error: unknown) => unexpectedSessionFailure(error));
    this.#writeQueue = write;
    return write;
  }

  #releaseOnce(): void {
    if (!this.#released) {
      this.#released = true;
      this.#release();
    }
  }
}


async function websocketMessageBytes(
  data: MessageEvent["data"],
  frameCap: number,
): Promise<WebSocketDecodeOutcome> {
  if (data instanceof ArrayBuffer) {
    return data.byteLength > frameCap
      ? frameTooLarge(data.byteLength, frameCap)
      : Tag("Decoded", new Uint8Array(data));
  }
  if (ArrayBuffer.isView(data)) {
    return data.byteLength > frameCap
      ? frameTooLarge(data.byteLength, frameCap)
      : Tag(
          "Decoded",
          new Uint8Array(data.buffer, data.byteOffset, data.byteLength),
        );
  }
  if (typeof Blob !== "undefined" && data instanceof Blob) {
    if (data.size > frameCap) {
      return frameTooLarge(data.size, frameCap);
    }
    try {
      return Tag("Decoded", new Uint8Array(await data.arrayBuffer()));
    } catch (error) {
      return Tag("TransferFailed", {
        direction: "Inbound",
        detail: describeHostError(error),
      });
    }
  }
  return Tag("UnsupportedFrame", {
    format: typeof data === "string" ? "Text" : "Unknown",
  });
}

function frameTooLarge(
  length: number,
  maximum: number,
): Extract<InterfaceSessionFailure, Tag<"FrameTooLarge", unknown>> {
  return Tag("FrameTooLarge", { length, maximum });
}

export function closeBrowserWebSocket(
  socket: WebSocket | undefined,
): InterfaceCleanupFailure | undefined {
  try {
    if (
      socket &&
      (socket.readyState === WEBSOCKET_CONNECTING ||
        socket.readyState === WEBSOCKET_OPEN)
    ) {
      socket.close();
    }
  } catch (error) {
    return Tag("TransportCloseFailed", {
      detail: describeHostError(error),
    });
  }
  return undefined;
}

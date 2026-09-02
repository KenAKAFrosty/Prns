import { Tag, match, match_into } from "../../casework.js";
import type { InterfaceId } from "../../contract.js";
import { bytesField, record, stringField } from "../decoding.js";
import { describeHostError } from "../host_errors.js";
import {
  closeFailed,
  closedSessionOutcome,
  delay,
  describeInterfaceSessionFailure,
  hasCleanupFailures,
  unexpectedSessionFailure,
} from "../session.js";
import { PrnsValidationError, packetFrameView } from "../values.js";
import type {
  InterfaceCleanupFailure,
  InterfaceCloseOutcome,
  InterfaceSessionFailure,
  InterfaceSessionStatus,
} from "../interface_contract.js";
import type { UsbAutoSession } from "./index.js";
import type { UsbAutoHostDecoder, UsbAutoRuntimeHost } from "./runtime.js";
import { WebUsbAutoTransport } from "./transport.js";
import type { UsbAutoWriteOutcome } from "./transport.js";

type UsbAutoInboundMessage =
  | Tag<"Hello">
  | Tag<"HelloAck", Uint8Array>
  | Tag<"Data", Uint8Array>;

type SessionHandleOutcome = Tag<"Handled"> | InterfaceSessionFailure;
type RawUsbAutoMessageType = "hello" | "helloAck" | "data";

const PROBE_INTERVAL_MS = 500;
const RAW_MESSAGE_TYPES: ReadonlySet<string> =
  new Set<RawUsbAutoMessageType>(["hello", "helloAck", "data"]);

export class BrowserUsbAutoSession implements UsbAutoSession {
  readonly name = "usb-auto" as const;
  readonly interfaceId: InterfaceId;

  readonly #host: UsbAutoRuntimeHost;
  readonly #transport: WebUsbAutoTransport;
  readonly #decoder: UsbAutoHostDecoder;
  readonly #nodeTag: Uint8Array;
  #writeQueue: Promise<UsbAutoWriteOutcome> = Promise.resolve(Tag("Written"));
  #closed = false;
  #confirmed = false;
  #status: InterfaceSessionStatus = Tag("Negotiating");
  #closePromise: Promise<InterfaceCloseOutcome> | undefined;

  constructor(
    host: UsbAutoRuntimeHost,
    transport: WebUsbAutoTransport,
    interfaceId: InterfaceId,
  ) {
    this.#host = host;
    this.#transport = transport;
    this.interfaceId = interfaceId;
    this.#decoder = host.createUsbAutoDecoder();
    this.#nodeTag = host.usbAutoNodeTagFor(interfaceId);
  }

  get status(): InterfaceSessionStatus {
    return this.#status;
  }

  start(): void {
    void this.#readLoop();
    void this.#probeLoop();
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
    this.#decoder.release?.();
    const causes: InterfaceCleanupFailure[] = [];
    const detached = await this.#host.deactivateInterface(this.interfaceId);
    if (detached.tag !== "Detached") {
      causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
    }
    const pendingWrite = await this.#writeQueue;
    if (pendingWrite.tag !== "Written") {
      causes.push(
        Tag("TransportCloseFailed", {
          detail: describeInterfaceSessionFailure(pendingWrite),
        }),
      );
    }
    causes.push(...(await this.#transport.close()));
    if (hasCleanupFailures(causes)) {
      const failed = closeFailed(causes);
      this.#status = Tag("Failed", failed);
      return failed;
    }
    this.#status = Tag("Closed");
    return Tag("Closed");
  }

  async #readLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const read = await this.#transport.read();
        if (read.tag !== "Read") {
          await this.#fail(read);
          return;
        }
        const chunk = read.data;
        if (!chunk) {
          break;
        }
        if (chunk.length === 0) {
          continue;
        }
        let messages: unknown[];
        try {
          const decoded = await this.#decoder.feed(chunk);
          if (!Array.isArray(decoded)) {
            await this.#fail(decoded);
            return;
          }
          messages = decoded;
        } catch (error) {
          await this.#fail(
            Tag("ProtocolViolation", {
              protocol: "UsbAuto",
              detail: describeHostError(error),
            }),
          );
          return;
        }
        for (const raw of messages) {
          let message: UsbAutoInboundMessage;
          try {
            message = parseUsbAutoMessage(raw);
          } catch (error) {
            await this.#fail(
              Tag("ProtocolViolation", {
                protocol: "UsbAuto",
                detail: describeHostError(error),
              }),
            );
            return;
          }
          const handled = await this.#handleInbound(message);
          if (handled.tag !== "Handled") {
            await this.#fail(handled);
            return;
          }
        }
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    } finally {
      if (!this.#closed) {
        await this.close();
      }
    }
  }

  async #probeLoop(): Promise<void> {
    try {
      while (!this.#closed && !this.#confirmed) {
        const written = await this.#writeFrame(this.#host.usbAutoHostHelloFrame());
        if (written.tag !== "Written") {
          await this.#fail(written);
          return;
        }
        await delay(PROBE_INTERVAL_MS);
      }
    } catch (error) {
      if (!this.#closed) {
        await this.#fail(unexpectedSessionFailure(error));
      }
    }
  }

  async #outboundLoop(): Promise<void> {
    try {
      while (!this.#closed) {
        const outbound = await this.#host.nextOutboundFor(this.interfaceId);
        if (outbound.tag === "InterfaceDetached") {
          return;
        }
        if (outbound.tag !== "Outbound") {
          await this.#fail(outbound);
          return;
        }
        for (const frame of outbound.data) {
          const written = await this.#writeFrame(
            this.#host.usbAutoDataFrame(frame.bytes),
          );
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

  async #handleInbound(message: UsbAutoInboundMessage): Promise<SessionHandleOutcome> {
    return match_into<Promise<SessionHandleOutcome>>().from(message, {
      Hello: async () => {
        const written = await this.#writeFrame(
          this.#host.usbAutoHostHelloAckFrame(this.#nodeTag),
        );
        if (written.tag !== "Written") {
          return written;
        }
        this.#confirmPeer();
        return Tag("Handled");
      },
      HelloAck: async () => {
        this.#confirmPeer();
        return Tag("Handled");
      },
      Data: async (bytes) => {
        if (this.#confirmed && bytes.length > 0) {
          const ingested = await this.#host.ingest(
            this.interfaceId,
            packetFrameView(bytes),
          );
          return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
        }
        return Tag("Handled");
      },
    });
  }

  #confirmPeer(): void {
    if (this.#confirmed) {
      return;
    }
    this.#confirmed = true;
    this.#status = Tag("Active");
    void this.#outboundLoop();
  }

  async #fail(sessionFailure: InterfaceSessionFailure): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#status = Tag("Failed", sessionFailure);
    this.#closed = true;
    this.#decoder.release?.();
    await this.#host.deactivateInterface(this.interfaceId);
    await this.#writeQueue;
    await this.#transport.close();
  }

  async #writeFrame(frame: Uint8Array): Promise<UsbAutoWriteOutcome> {
    if (this.#closed) {
      return Tag("Written");
    }
    const write = this.#writeQueue
      .then(async (previous): Promise<UsbAutoWriteOutcome> => {
        if (previous.tag !== "Written" || this.#closed) {
          return previous;
        }
        return this.#transport.write(frame);
      })
      .catch((error: unknown) => unexpectedSessionFailure(error));
    this.#writeQueue = write;
    return write;
  }
}

function parseUsbAutoMessage(raw: unknown): UsbAutoInboundMessage {
  const object = record(raw, "UsbAutoInboundMessage");
  const type = stringField(object, "type");
  if (!RAW_MESSAGE_TYPES.has(type)) {
    throw new PrnsValidationError(
      "invalid-component",
      `unknown USB-auto message ${type}`,
    );
  }
  return match(type as RawUsbAutoMessageType, {
    hello: () => Tag("Hello"),
    helloAck: () => Tag("HelloAck", bytesField(object, "tag")),
    data: () => Tag("Data", bytesField(object, "bytes")),
  });
}

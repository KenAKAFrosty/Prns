import { Tag, match, match_into } from "../../casework.js";
import { bytesField, record, stringField } from "../decoding.js";
import { describeHostError } from "../host_errors.js";
import { closeFailed, closedSessionOutcome, delay, describeInterfaceSessionFailure, hasCleanupFailures, unexpectedSessionFailure, } from "../session.js";
import { PrnsValidationError, packetFrame } from "../values.js";
import { WebUsbAutoTransport } from "./transport.js";
const PROBE_INTERVAL_MS = 500;
const OUTBOUND_POLL_MS = 25;
const RAW_MESSAGE_TYPES = new Set(["hello", "helloAck", "data"]);
export class BrowserUsbAutoSession {
    name = "usb-auto";
    interfaceId;
    #host;
    #transport;
    #decoder;
    #nodeTag;
    #writeQueue = Promise.resolve(Tag("Written"));
    #closed = false;
    #confirmed = false;
    #status = Tag("Negotiating");
    constructor(host, transport, interfaceId) {
        this.#host = host;
        this.#transport = transport;
        this.interfaceId = interfaceId;
        this.#decoder = host.createUsbAutoDecoder();
        this.#nodeTag = host.usbAutoNodeTagFor(interfaceId);
    }
    get status() {
        return this.#status;
    }
    start() {
        void this.#readLoop();
        void this.#probeLoop();
        void this.#outboundLoop();
    }
    async close() {
        if (this.#closed) {
            return closedSessionOutcome(this.#status);
        }
        this.#closed = true;
        const causes = [];
        const detached = this.#host.deactivateInterface(this.interfaceId);
        if (detached.tag !== "Detached") {
            causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
        }
        const pendingWrite = await this.#writeQueue;
        if (pendingWrite.tag !== "Written") {
            causes.push(Tag("TransportCloseFailed", {
                detail: describeInterfaceSessionFailure(pendingWrite),
            }));
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
    async #readLoop() {
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
                let messages;
                try {
                    messages = this.#decoder.feed(chunk);
                }
                catch (error) {
                    await this.#fail(Tag("ProtocolViolation", {
                        protocol: "UsbAuto",
                        detail: describeHostError(error),
                    }));
                    return;
                }
                for (const raw of messages) {
                    let message;
                    try {
                        message = parseUsbAutoMessage(raw);
                    }
                    catch (error) {
                        await this.#fail(Tag("ProtocolViolation", {
                            protocol: "UsbAuto",
                            detail: describeHostError(error),
                        }));
                        return;
                    }
                    const handled = await this.#handleInbound(message);
                    if (handled.tag !== "Handled") {
                        await this.#fail(handled);
                        return;
                    }
                }
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(unexpectedSessionFailure(error));
            }
        }
        finally {
            if (!this.#closed) {
                await this.close();
            }
        }
    }
    async #probeLoop() {
        try {
            while (!this.#closed && !this.#confirmed) {
                const written = await this.#writeFrame(this.#host.usbAutoHostHelloFrame());
                if (written.tag !== "Written") {
                    await this.#fail(written);
                    return;
                }
                await delay(PROBE_INTERVAL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(unexpectedSessionFailure(error));
            }
        }
    }
    async #outboundLoop() {
        try {
            while (!this.#closed) {
                if (this.#confirmed) {
                    const outbound = this.#host.takeOutboundFor(this.interfaceId);
                    if (outbound.tag !== "Outbound") {
                        await this.#fail(outbound);
                        return;
                    }
                    for (const frame of outbound.data) {
                        const written = await this.#writeFrame(this.#host.usbAutoDataFrame(frame.bytes));
                        if (written.tag !== "Written") {
                            await this.#fail(written);
                            return;
                        }
                    }
                }
                await delay(OUTBOUND_POLL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(unexpectedSessionFailure(error));
            }
        }
    }
    async #handleInbound(message) {
        return match_into().from(message, {
            Hello: async () => {
                const written = await this.#writeFrame(this.#host.usbAutoHostHelloAckFrame(this.#nodeTag));
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
                    const ingested = this.#host.ingest(this.interfaceId, packetFrame(bytes));
                    return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
                }
                return Tag("Handled");
            },
        });
    }
    #confirmPeer() {
        this.#confirmed = true;
        this.#status = Tag("Active");
    }
    async #fail(sessionFailure) {
        if (this.#closed) {
            return;
        }
        this.#status = Tag("Failed", sessionFailure);
        this.#closed = true;
        this.#host.deactivateInterface(this.interfaceId);
        await this.#writeQueue;
        await this.#transport.close();
    }
    async #writeFrame(frame) {
        if (this.#closed) {
            return Tag("Written");
        }
        const write = this.#writeQueue
            .then(async (previous) => {
            if (previous.tag !== "Written" || this.#closed) {
                return previous;
            }
            return this.#transport.write(frame);
        })
            .catch((error) => unexpectedSessionFailure(error));
        this.#writeQueue = write;
        return write;
    }
}
function parseUsbAutoMessage(raw) {
    const object = record(raw, "UsbAutoInboundMessage");
    const type = stringField(object, "type");
    if (!RAW_MESSAGE_TYPES.has(type)) {
        throw new PrnsValidationError("invalid-component", `unknown USB-auto message ${type}`);
    }
    return match(type, {
        hello: () => Tag("Hello"),
        helloAck: () => Tag("HelloAck", bytesField(object, "tag")),
        data: () => Tag("Data", bytesField(object, "bytes")),
    });
}

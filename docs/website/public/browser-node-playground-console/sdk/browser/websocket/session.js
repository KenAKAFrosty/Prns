import { Tag } from "../../casework.js";
import { describeHostError } from "../host_errors.js";
import { closeFailed, closedSessionOutcome, delay, describeInterfaceSessionFailure, hasCleanupFailures, unexpectedSessionFailure, } from "../session.js";
const OUTBOUND_POLL_MS = 25;
const BUFFER_POLL_MS = 4;
const MIN_BUFFER_LIMIT = 1024 * 1024;
const WEBSOCKET_CONNECTING = 0;
const WEBSOCKET_OPEN = 1;
export class BrowserWebSocketSession {
    name = "websocket";
    interfaceId;
    url;
    framing;
    #host;
    #socket;
    #frameCap;
    #codec;
    #bufferLimit;
    #release;
    #readQueue = Promise.resolve();
    #writeQueue = Promise.resolve(Tag("Written"));
    #closed = false;
    #released = false;
    #status = Tag("Active");
    constructor(host, socket, interfaceId, url, frameCap, framing, codec, release) {
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
    get status() {
        return this.#status;
    }
    start() {
        this.#socket.addEventListener("message", (event) => {
            this.#enqueueMessage(event);
        });
        this.#socket.addEventListener("close", () => {
            this.#handleClose();
        });
        this.#socket.addEventListener("error", () => {
            void this.#fail(Tag("Disconnected", {
                detail: `WebSocket connection failed for ${this.url}`,
            }));
        });
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
        this.#releaseOnce();
        const socketFailure = closeBrowserWebSocket(this.#socket);
        if (socketFailure) {
            causes.push(socketFailure);
        }
        const pendingWrite = await this.#writeQueue;
        if (pendingWrite.tag !== "Written") {
            causes.push(Tag("TransportCloseFailed", {
                detail: describeInterfaceSessionFailure(pendingWrite),
            }));
        }
        if (hasCleanupFailures(causes)) {
            const failed = closeFailed(causes);
            this.#status = Tag("Failed", failed);
            return failed;
        }
        this.#status = Tag("Closed");
        return Tag("Closed");
    }
    #enqueueMessage(event) {
        this.#readQueue = this.#readQueue
            .then(async () => {
            const handled = await this.#handleMessage(event);
            if (handled.tag !== "Handled" && !this.#closed) {
                await this.#fail(handled);
            }
        })
            .catch(async (error) => {
            if (!this.#closed) {
                await this.#fail(unexpectedSessionFailure(error));
            }
        });
    }
    async #handleMessage(event) {
        const decoded = await websocketMessageBytes(event.data, this.#codec.messageCap());
        if (decoded.tag !== "Decoded") {
            return decoded;
        }
        const batch = this.#codec.decode(decoded.data);
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
    #handleClose() {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        const detached = this.#host.deactivateInterface(this.interfaceId);
        this.#status =
            detached.tag === "Detached" ? Tag("Closed") : Tag("Failed", detached);
        this.#releaseOnce();
    }
    async #outboundLoop() {
        try {
            while (!this.#closed) {
                if (this.#codec.rawFallbackIsArmed()) {
                    await delay(this.#codec.rawFallbackDelayMillis());
                    if (this.#closed) {
                        return;
                    }
                    if (this.#codec.rawFallbackIsArmed()) {
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
                    await delay(OUTBOUND_POLL_MS);
                    continue;
                }
                const maximumFrames = this.#codec.canStageMultipleOutbound()
                    ? Number.MAX_SAFE_INTEGER
                    : 1;
                const outbound = this.#host.takeOutboundFor(this.interfaceId, maximumFrames);
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
                await delay(OUTBOUND_POLL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(unexpectedSessionFailure(error));
            }
        }
    }
    async #fail(sessionFailure) {
        if (this.#closed) {
            return;
        }
        this.#status = Tag("Failed", sessionFailure);
        this.#closed = true;
        this.#host.deactivateInterface(this.interfaceId);
        this.#releaseOnce();
        await this.#writeQueue;
        closeBrowserWebSocket(this.#socket);
    }
    async #writeFrame(frame) {
        if (this.#closed || frame.length === 0) {
            return Tag("Written");
        }
        if (frame.length > this.#frameCap) {
            return Tag("FrameTooLarge", {
                length: frame.length,
                maximum: this.#frameCap,
            });
        }
        let encoded;
        try {
            encoded = this.#codec.stageOutbound(frame);
        }
        catch (error) {
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
    async #writeEncodedFrame(frame) {
        if (this.#closed || frame.length === 0) {
            return Tag("Written");
        }
        const write = this.#writeQueue
            .then(async (previous) => {
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
            }
            catch (error) {
                return Tag("Disconnected", { detail: describeHostError(error) });
            }
        })
            .catch((error) => unexpectedSessionFailure(error));
        this.#writeQueue = write;
        return write;
    }
    #releaseOnce() {
        if (!this.#released) {
            this.#released = true;
            this.#release();
        }
    }
}
async function websocketMessageBytes(data, frameCap) {
    if (data instanceof ArrayBuffer) {
        return data.byteLength > frameCap
            ? frameTooLarge(data.byteLength, frameCap)
            : Tag("Decoded", new Uint8Array(data));
    }
    if (ArrayBuffer.isView(data)) {
        return data.byteLength > frameCap
            ? frameTooLarge(data.byteLength, frameCap)
            : Tag("Decoded", new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
    }
    if (typeof Blob !== "undefined" && data instanceof Blob) {
        if (data.size > frameCap) {
            return frameTooLarge(data.size, frameCap);
        }
        try {
            return Tag("Decoded", new Uint8Array(await data.arrayBuffer()));
        }
        catch (error) {
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
function frameTooLarge(length, maximum) {
    return Tag("FrameTooLarge", { length, maximum });
}
export function closeBrowserWebSocket(socket) {
    try {
        if (socket &&
            (socket.readyState === WEBSOCKET_CONNECTING ||
                socket.readyState === WEBSOCKET_OPEN)) {
            socket.close();
        }
    }
    catch (error) {
        return Tag("TransportCloseFailed", {
            detail: describeHostError(error),
        });
    }
    return undefined;
}

import { Tag } from "../../casework.js";
import { byteKey } from "../bytes.js";
import { connectFailure, describeHostError } from "../host_errors.js";
import { BrowserWebSocketSession, closeBrowserWebSocket, } from "./session.js";
const CONNECT_TIMEOUT_MS = 10_000;
const DEFAULT_FRAMING_SELECTION = "Auto";
export const BROWSER_RENDEZVOUS_FRAMING_SELECTION = "RawPacket";
export class WebSocketInterface {
    name = "websocket";
    #host;
    #activeTags = new Set();
    constructor(host) {
        this.#host = host;
    }
    async connect(url, options = {}) {
        const ready = this.#host.runtimeReadiness();
        if (ready.tag !== "Ready") {
            return ready;
        }
        const canonical = canonicalWebSocketUrl(url);
        if (canonical.tag !== "Canonical") {
            return canonical;
        }
        const target = canonical.data;
        const protocols = normalizedWebSocketProtocols(options.protocols);
        const framing = options.framing ?? DEFAULT_FRAMING_SELECTION;
        let tag;
        let codec;
        try {
            tag =
                options.channelTag ??
                    browserWebSocketChannelTag(target, protocols, framing);
            codec = this.#host.createWebSocketFramingCodec(framing);
        }
        catch (error) {
            return connectFailure("websocket", "RuntimeRegistration", error);
        }
        const tagKey = byteKey(tag);
        if (this.#activeTags.has(tagKey)) {
            return Tag("AlreadyActive", { interface: "websocket", target });
        }
        this.#activeTags.add(tagKey);
        let socket;
        let interfaceId;
        let stage = "TransportOpen";
        try {
            const opened = await openBrowserWebSocket(target, protocols);
            if (opened.tag !== "Opened") {
                this.#activeTags.delete(tagKey);
                return opened;
            }
            socket = opened.data;
            stage = "RuntimeRegistration";
            const registered = this.#host.webSocketRegister({
                channelTag: tag,
                bitrateBps: options.bitrateBps ?? this.#host.websocketBitrateBps(),
                hardwareMtu: options.hardwareMtu ?? this.#host.websocketHardwareMtu(),
                ...(options.routing === undefined ? {} : { routing: options.routing }),
            });
            if (registered.tag !== "Registered") {
                closeBrowserWebSocket(socket);
                this.#activeTags.delete(tagKey);
                return registered;
            }
            interfaceId = registered.data;
            stage = "Handshake";
            const session = new BrowserWebSocketSession(this.#host, socket, interfaceId, target, this.#host.websocketFrameCap(), framing, codec, () => this.#activeTags.delete(tagKey));
            session.start();
            return Tag("Connected", session);
        }
        catch (error) {
            if (interfaceId) {
                this.#host.deactivateInterface(interfaceId);
            }
            closeBrowserWebSocket(socket);
            this.#activeTags.delete(tagKey);
            return connectFailure("websocket", stage, error);
        }
    }
}
function requireBrowserWebSocket() {
    try {
        const WebSocketCtor = globalThis.WebSocket;
        return WebSocketCtor
            ? Tag("Available", WebSocketCtor)
            : Tag("HostApiUnavailable", { api: "WebSocket" });
    }
    catch {
        return Tag("HostApiUnavailable", { api: "WebSocket" });
    }
}
async function openBrowserWebSocket(url, protocols) {
    const available = requireBrowserWebSocket();
    if (available.tag !== "Available") {
        return available;
    }
    const protocolList = protocols === undefined || typeof protocols === "string"
        ? protocols
        : [...protocols];
    let socket;
    try {
        const WebSocketCtor = available.data;
        socket =
            protocolList === undefined
                ? new WebSocketCtor(url)
                : new WebSocketCtor(url, protocolList);
    }
    catch (error) {
        return connectFailure("websocket", "TransportOpen", error);
    }
    try {
        socket.binaryType = "arraybuffer";
    }
    catch (error) {
        closeBrowserWebSocket(socket);
        return connectFailure("websocket", "TransportOpen", error);
    }
    return new Promise((resolve) => {
        let timeout;
        const cleanup = () => {
            if (timeout !== undefined) {
                globalThis.clearTimeout(timeout);
            }
            socket.removeEventListener("open", handleOpen);
            socket.removeEventListener("error", handleError);
            socket.removeEventListener("close", handleClose);
        };
        const handleOpen = () => {
            cleanup();
            resolve(Tag("Opened", socket));
        };
        const handleError = () => {
            cleanup();
            closeBrowserWebSocket(socket);
            resolve(Tag("ConnectionFailed", {
                interface: "websocket",
                stage: "TransportOpen",
                detail: `WebSocket connection failed for ${url}`,
            }));
        };
        const handleClose = () => {
            cleanup();
            resolve(Tag("ConnectionFailed", {
                interface: "websocket",
                stage: "TransportOpen",
                detail: `WebSocket connection closed before opening for ${url}`,
            }));
        };
        const handleTimeout = () => {
            cleanup();
            closeBrowserWebSocket(socket);
            resolve(Tag("TimedOut", {
                interface: "websocket",
                stage: "TransportOpen",
                timeoutMs: CONNECT_TIMEOUT_MS,
            }));
        };
        try {
            timeout = globalThis.setTimeout(handleTimeout, CONNECT_TIMEOUT_MS);
            socket.addEventListener("open", handleOpen);
            socket.addEventListener("error", handleError);
            socket.addEventListener("close", handleClose);
        }
        catch (error) {
            cleanup();
            closeBrowserWebSocket(socket);
            resolve(connectFailure("websocket", "TransportOpen", error));
        }
    });
}
function canonicalWebSocketUrl(url) {
    let target;
    try {
        target = new URL(url.toString());
    }
    catch (error) {
        return Tag("InvalidTarget", {
            interface: "websocket",
            target: url.toString(),
            detail: describeHostError(error),
        });
    }
    if (target.protocol !== "ws:" && target.protocol !== "wss:") {
        return Tag("InvalidTarget", {
            interface: "websocket",
            target: target.toString(),
            detail: "WebSocket URL must use the ws or wss scheme",
        });
    }
    return Tag("Canonical", target.toString());
}
function normalizedWebSocketProtocols(protocols) {
    if (protocols === undefined || typeof protocols === "string") {
        return protocols;
    }
    return protocols.length === 0 ? undefined : [...protocols];
}
function browserWebSocketChannelTag(url, protocols, framing) {
    const protocolList = protocols === undefined
        ? []
        : typeof protocols === "string"
            ? [protocols]
            : protocols;
    return new TextEncoder().encode(JSON.stringify(["websocket-client", url, protocolList, framing]));
}

import { Tag, from, match, match_into } from "../casework.js";
import { BoundedAsyncLane } from "../async_lanes.js";
import { DESTINATION_HASH_LENGTH, HOST_CONTRACT_ABI, INTERFACE_ID_LENGTH, PRODUCT_VERSION, RESOURCE_HASH_LENGTH, balancedLimits, destinationHash, identityHash, interfaceId, linkId, packetHash, requestId, requestPathHash, resourceHash, } from "../contract.js";
import { MemoryResourceStream } from "../memory_resource.js";
import { AutoWifiInterface } from "./auto_wifi.js";
import { blobResourceSource, byteResourceSource, sendResourceFromSource, } from "./resource_send.js";
import { browserResourceCompressor } from "./resource_compressor.js";
export { Tag, from, match, match_into };
export { DESTINATION_HASH_LENGTH, HOST_CONTRACT_ABI, INTERFACE_ID_LENGTH, PRODUCT_VERSION, RESOURCE_HASH_LENGTH, balancedLimits, destinationHash, identityHash, interfaceId, linkId, packetHash, requestId, requestPathHash, resourceHash, };
export { AutoWifiController, AutoWifiInterface, parseBrowserGatewayCatalog, validateBrowserGatewayUrl, } from "./auto_wifi.js";
export const MIN_ENTROPY_BYTES = 128;
export const BLE_IDENTITY_LENGTH = 16;
export class PrnsValidationError extends Error {
    code;
    constructor(code, message) {
        super(message);
        this.name = "PrnsValidationError";
        this.code = code;
    }
}
const RAW_EVENT_TYPES = new Set([
    "announce",
    "selfRatchetRotated",
    "announceHeldDropped",
    "commandSettled",
    "linkEstablished",
    "peerIdentified",
    "request",
    "response",
    "responseSegment",
    "channelMessage",
    "singleDelivery",
    "delivered",
    "linkClosed",
    "linkInterfaceMismatch",
    "resourceReceived",
    "resourceFailed",
    "resourceNeedsDecompression",
    "resourceSegment",
    "resourceAssembled",
    "routeExpired",
    "routeEvicted",
    "routeInterfaceGone",
    "routeDropped",
]);
const USB_AUTO_PROBE_INTERVAL_MS = 500;
const USB_AUTO_OUTBOUND_POLL_MS = 25;
const WEBUSB_MIN_TRANSFER_BYTES = 512;
const BLUETOOTH_HANDSHAKE_TIMEOUT_MS = 10_000;
const BLUETOOTH_OUTBOUND_POLL_MS = 25;
const WEBSOCKET_CONNECT_TIMEOUT_MS = 10_000;
const WEBSOCKET_OUTBOUND_POLL_MS = 25;
const WEBSOCKET_BUFFER_POLL_MS = 4;
const WEBSOCKET_MIN_BUFFER_LIMIT = 1024 * 1024;
const WEBSOCKET_CONNECTING = 0;
const WEBSOCKET_OPEN = 1;
const INTERFACE_OUTBOUND_QUEUE_DEPTH = 64;
let nextBrowserUsbAutoTag = 0;
const LINUX_WEBUSB_SETUP_HINT = "On Linux, run ./tools/prns device webusb install from the Prns repo root, " +
    "then unplug/replug the device and restart the browser. If this is Snap Chromium, " +
    "also run sudo snap connect chromium:raw-usb or use a non-Snap Chrome/Chromium build.";
export class BrowserLocalStorageIdentityStore {
    #key;
    constructor(key = "prns.identity.v1") {
        this.#key = key;
    }
    async load(expectedLength) {
        let encoded;
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().atob) {
                return Tag("HostApiUnavailable", { api: "Base64Decoder" });
            }
            encoded = storage.getItem(this.#key);
        }
        catch (error) {
            return Tag("IdentityStoreFailed", {
                operation: "Load",
                detail: describeHostError(error),
            });
        }
        if (encoded === null) {
            return Tag("Missing");
        }
        try {
            return Tag("Loaded", identitySecretKey(decodeBase64(encoded), expectedLength));
        }
        catch (error) {
            return Tag("StoredIdentityInvalid", {
                detail: describeHostError(error),
            });
        }
    }
    async save(secretKey) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().btoa) {
                return Tag("HostApiUnavailable", { api: "Base64Encoder" });
            }
            storage.setItem(this.#key, encodeBase64(secretKey));
            return Tag("Saved");
        }
        catch (error) {
            return Tag("IdentityStoreFailed", {
                operation: "Save",
                detail: describeHostError(error),
            });
        }
    }
}
export class BrowserLocalStorageBleIdentityStore {
    #key;
    constructor(key = "prns.ble-identity.v1") {
        this.#key = key;
    }
    async load(expectedLength) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().atob) {
                return Tag("HostApiUnavailable", { api: "Base64Decoder" });
            }
            const encoded = storage.getItem(this.#key);
            if (encoded === null) {
                return Tag("Missing");
            }
            const bytes = decodeBase64(encoded);
            if (bytes.length !== expectedLength) {
                return Tag("StoredStableIdentityInvalid", {
                    detail: `stored Bluetooth LE identity has ${bytes.length} bytes; expected ${expectedLength}`,
                });
            }
            return Tag("Loaded", bytes);
        }
        catch (error) {
            return Tag("StableIdentityStoreFailed", {
                operation: "Load",
                detail: describeHostError(error),
            });
        }
    }
    async save(identity) {
        try {
            const storage = hostGlobal().localStorage;
            if (!storage) {
                return Tag("HostApiUnavailable", { api: "LocalStorage" });
            }
            if (!hostGlobal().btoa) {
                return Tag("HostApiUnavailable", { api: "Base64Encoder" });
            }
            storage.setItem(this.#key, encodeBase64(identity));
            return Tag("Saved");
        }
        catch (error) {
            return Tag("StableIdentityStoreFailed", {
                operation: "Save",
                detail: describeHostError(error),
            });
        }
    }
}
export class PrnsInterfaces {
    usbAuto;
    rnode;
    bluetooth;
    autoWifi;
    webSocket;
    constructor(host) {
        this.usbAuto = new UsbAutoInterface(host);
        this.rnode = new RNodeInterface(host);
        this.bluetooth = new BluetoothInterface(host);
        this.autoWifi = new AutoWifiInterface(host);
        this.webSocket = new WebSocketInterface(host);
    }
}
export class UsbAutoInterface {
    name = "usb-auto";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect(options = {}) {
        const ready = this.#host.runtimeReadiness();
        if (ready.tag !== "Ready") {
            return ready;
        }
        const available = requireWebUsb();
        if (available.tag !== "Available") {
            return available;
        }
        let transport;
        let interfaceId;
        let stage = "DeviceSelection";
        try {
            const requested = await usbStage("DeviceSelection", "request device", () => available.data.requestDevice({
                filters: options.filters ?? this.#host.defaultUsbAutoFilters(),
            }));
            if (requested.tag !== "Completed") {
                return requested;
            }
            stage = "TransportOpen";
            const opened = await WebUsbAutoTransport.open(requested.data);
            if (opened.tag !== "Opened") {
                return opened;
            }
            transport = opened.data;
            stage = "RuntimeRegistration";
            const registered = this.#host.registerInterface({
                interfaceName: "usb-auto",
                kind: "auto-usb-host",
                channelTag: browserUsbAutoChannelTag(requested.data),
                bitrateBps: this.#host.usbAutoHostBitrateBps(),
                hardwareMtu: this.#host.usbAutoHostHardwareMtu(),
            });
            if (registered.tag !== "Registered") {
                await transport.close();
                return registered;
            }
            interfaceId = registered.data;
            stage = "Handshake";
            const session = new BrowserUsbAutoSession(this.#host, transport, interfaceId);
            session.start();
            return Tag("Connected", session);
        }
        catch (error) {
            if (interfaceId) {
                this.#host.deactivateInterface(interfaceId);
            }
            await transport?.close();
            return connectFailure("usb-auto", stage, error);
        }
    }
}
class BrowserUsbAutoSession {
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
                await delay(USB_AUTO_PROBE_INTERVAL_MS);
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
                await delay(USB_AUTO_OUTBOUND_POLL_MS);
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
class WebUsbAutoTransport {
    #device;
    #interfaceNumber;
    #inEndpoint;
    #outEndpoint;
    #closed = false;
    constructor(device, interfaceNumber, inEndpoint, outEndpoint) {
        this.#device = device;
        this.#interfaceNumber = interfaceNumber;
        this.#inEndpoint = inEndpoint;
        this.#outEndpoint = outEndpoint;
    }
    static async open(device) {
        const opened = await usbStage("TransportOpen", "open selected device", () => device.open());
        if (opened.tag !== "Completed") {
            return opened;
        }
        const configured = firstUsbConfiguration(device);
        if (configured.tag !== "Configured") {
            await closeUsbDevice(device);
            return configured;
        }
        const configuration = device.configuration ?? configured.data;
        if (!device.configuration) {
            const selected = await usbStage("TransportOpen", `select configuration ${configuration.configurationValue}`, () => device.selectConfiguration(configuration.configurationValue));
            if (selected.tag !== "Completed") {
                await closeUsbDevice(device);
                return selected;
            }
        }
        const selectedConfiguration = device.configuration ?? configured.data;
        const endpoints = findWebUsbEndpointPair(selectedConfiguration);
        if (!endpoints) {
            await closeUsbDevice(device);
            return Tag("UnsupportedDevice", {
                interface: "usb-auto",
                capability: "usable IN/OUT endpoint pair",
            });
        }
        const claimed = await usbStage("TransportOpen", `claim interface ${endpoints.interfaceNumber}`, () => device.claimInterface(endpoints.interfaceNumber));
        if (claimed.tag !== "Completed") {
            await closeUsbDevice(device);
            return claimed;
        }
        if (endpoints.alternate.alternateSetting !== 0 &&
            device.selectAlternateInterface) {
            const selected = await usbStage("TransportOpen", `select alternate ${endpoints.alternate.alternateSetting} ` +
                `on interface ${endpoints.interfaceNumber}`, () => device.selectAlternateInterface(endpoints.interfaceNumber, endpoints.alternate.alternateSetting));
            if (selected.tag !== "Completed") {
                await closeUsbDevice(device);
                return selected;
            }
        }
        return Tag("Opened", new WebUsbAutoTransport(device, endpoints.interfaceNumber, endpoints.inEndpoint, endpoints.outEndpoint));
    }
    async read() {
        if (this.#closed) {
            return Tag("Read", undefined);
        }
        try {
            const length = Math.max(this.#inEndpoint.packetSize, WEBUSB_MIN_TRANSFER_BYTES);
            const result = await this.#device.transferIn(this.#inEndpoint.endpointNumber, length);
            if (result.status !== "ok") {
                return Tag("TransferFailed", {
                    direction: "Inbound",
                    detail: `USB transfer status ${result.status}`,
                });
            }
            const data = result.data;
            if (!data) {
                return Tag("Read", new Uint8Array());
            }
            return Tag("Read", new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength)));
        }
        catch (error) {
            return Tag("TransferFailed", {
                direction: "Inbound",
                detail: describeHostError(error),
            });
        }
    }
    async write(bytes) {
        if (this.#closed || bytes.length === 0) {
            return Tag("Written");
        }
        try {
            const result = await this.#device.transferOut(this.#outEndpoint.endpointNumber, arrayBufferForUsb(bytes));
            if (result.status !== "ok" || result.bytesWritten !== bytes.length) {
                return Tag("TransferFailed", {
                    direction: "Outbound",
                    detail: `wrote ${result.bytesWritten}/${bytes.length} bytes with status ${result.status}`,
                });
            }
            return Tag("Written");
        }
        catch (error) {
            return Tag("TransferFailed", {
                direction: "Outbound",
                detail: describeHostError(error),
            });
        }
    }
    async close() {
        if (this.#closed) {
            return [];
        }
        this.#closed = true;
        const failures = [];
        try {
            await this.#device.releaseInterface(this.#interfaceNumber);
        }
        catch (error) {
            failures.push(Tag("TransportCloseFailed", {
                detail: `release USB interface: ${describeHostError(error)}`,
            }));
        }
        try {
            await this.#device.close();
        }
        catch (error) {
            failures.push(Tag("TransportCloseFailed", {
                detail: `close USB device: ${describeHostError(error)}`,
            }));
        }
        return failures;
    }
}
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
        let tag;
        try {
            tag = options.channelTag ?? browserWebSocketChannelTag(target, protocols);
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
            const registered = this.#host.registerInterface({
                interfaceName: "websocket",
                kind: "websocket-client",
                channelTag: tag,
                bitrateBps: options.bitrateBps ?? this.#host.websocketBitrateBps(),
                hardwareMtu: options.hardwareMtu ?? this.#host.websocketHardwareMtu(),
            });
            if (registered.tag !== "Registered") {
                closeBrowserWebSocket(socket);
                this.#activeTags.delete(tagKey);
                return registered;
            }
            interfaceId = registered.data;
            stage = "Handshake";
            const session = new BrowserWebSocketSession(this.#host, socket, interfaceId, target, this.#host.websocketFrameCap(), () => this.#activeTags.delete(tagKey));
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
class BrowserWebSocketSession {
    name = "websocket";
    interfaceId;
    url;
    #host;
    #socket;
    #frameCap;
    #bufferLimit;
    #release;
    #readQueue = Promise.resolve();
    #writeQueue = Promise.resolve(Tag("Written"));
    #closed = false;
    #released = false;
    #status = Tag("Active");
    constructor(host, socket, interfaceId, url, frameCap, release) {
        this.#host = host;
        this.#socket = socket;
        this.interfaceId = interfaceId;
        this.url = url;
        this.#frameCap = frameCap;
        this.#bufferLimit = Math.max(WEBSOCKET_MIN_BUFFER_LIMIT, frameCap * 2);
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
        const decoded = await websocketMessageBytes(event.data, this.#frameCap);
        if (decoded.tag !== "Decoded") {
            return decoded;
        }
        if (decoded.data.length > 0 && !this.#closed) {
            const ingested = this.#host.ingest(this.interfaceId, packetFrame(decoded.data));
            return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
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
                const outbound = this.#host.takeOutboundFor(this.interfaceId);
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
                await delay(WEBSOCKET_OUTBOUND_POLL_MS);
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
        const write = this.#writeQueue
            .then(async (previous) => {
            if (previous.tag !== "Written" || this.#closed) {
                return previous;
            }
            while (!this.#closed && this.#socket.bufferedAmount > this.#bufferLimit) {
                await delay(WEBSOCKET_BUFFER_POLL_MS);
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
export class RNodeInterface {
    name = "rnode";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect() {
        const ready = this.#host.runtimeReadiness();
        if (ready.tag !== "Ready") {
            return ready;
        }
        return Tag("UnsupportedInterface", {
            interface: "rnode",
            host: "Browser",
        });
    }
}
export class BluetoothInterface {
    name = "bluetooth";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect() {
        const identity = this.#host.bluetoothIdentityReadiness();
        if (identity.tag !== "Ready") {
            return identity;
        }
        const ready = this.#host.runtimeReadiness();
        if (ready.tag !== "Ready") {
            return ready;
        }
        const available = requireWebBluetooth();
        if (available.tag !== "Available") {
            return available;
        }
        let server;
        let session;
        let stage = "DeviceSelection";
        try {
            const serviceUuid = this.#host.bluetoothServiceUuid();
            const requested = await bluetoothStage("DeviceSelection", () => available.data.requestDevice({
                filters: [{ services: [serviceUuid] }],
                optionalServices: [serviceUuid],
            }));
            if (requested.tag !== "Completed") {
                return requested;
            }
            const gatt = requested.data.gatt;
            if (!gatt) {
                return Tag("UnsupportedDevice", {
                    interface: "bluetooth",
                    capability: "GATT server",
                });
            }
            stage = "TransportOpen";
            const connected = await bluetoothStage("TransportOpen", () => gatt.connect());
            if (connected.tag !== "Completed") {
                return connected;
            }
            const connectedServer = connected.data;
            server = connectedServer;
            stage = "ServiceDiscovery";
            const discovered = await bluetoothStage("ServiceDiscovery", () => connectedServer.getPrimaryService(serviceUuid));
            if (discovered.tag !== "Completed") {
                disconnectBluetoothServer(connectedServer);
                return discovered;
            }
            const control = await bluetoothStage("ServiceDiscovery", () => discovered.data.getCharacteristic(this.#host.bluetoothControlUuid()));
            if (control.tag !== "Completed") {
                disconnectBluetoothServer(connectedServer);
                return control;
            }
            const data = await optionalBluetoothCharacteristic(discovered.data, this.#host.bluetoothDataUuid());
            stage = "Handshake";
            session = new BrowserBluetoothSession(this.#host, connectedServer, control.data, data ?? control.data);
            const started = await session.start();
            if (started.tag !== "Started") {
                await session.close();
                return started;
            }
            return Tag("Connected", session);
        }
        catch (error) {
            if (session) {
                await session.close();
            }
            else if (server) {
                disconnectBluetoothServer(server);
            }
            return connectFailure("bluetooth", stage, error);
        }
    }
}
class BrowserBluetoothSession {
    name = "bluetooth";
    #host;
    #server;
    #control;
    #data;
    #reassembler;
    #interfaceId;
    #writeQueue = Promise.resolve(Tag("Written"));
    #closed = false;
    #confirmed = false;
    #status = Tag("Negotiating");
    #connectFailure;
    constructor(host, server, control, data) {
        this.#host = host;
        this.#server = server;
        this.#control = control;
        this.#data = data;
        this.#reassembler = host.createBluetoothReassembler();
    }
    get interfaceId() {
        if (!this.#interfaceId) {
            throw new PrnsValidationError("invalid-component", "Bluetooth peer interface is not registered yet");
        }
        return this.#interfaceId;
    }
    get status() {
        return this.#status;
    }
    async start() {
        const controlStarted = await bluetoothStage("Handshake", () => this.#control.startNotifications());
        if (controlStarted.tag !== "Completed") {
            return controlStarted;
        }
        this.#control.addEventListener("characteristicvaluechanged", (event) => {
            try {
                const handled = this.#handleControlEvent(event);
                if (handled.tag !== "Handled") {
                    this.#handleEventFailure(handled);
                }
            }
            catch (error) {
                this.#handleEventFailure(unexpectedSessionFailure(error));
            }
        });
        if (this.#data !== this.#control) {
            const dataStarted = await bluetoothStage("Handshake", () => this.#data.startNotifications());
            if (dataStarted.tag !== "Completed") {
                return dataStarted;
            }
            this.#data.addEventListener("characteristicvaluechanged", (event) => {
                try {
                    const handled = this.#handleDataEvent(event);
                    if (handled.tag !== "Handled") {
                        this.#handleEventFailure(handled);
                    }
                }
                catch (error) {
                    this.#handleEventFailure(unexpectedSessionFailure(error));
                }
            });
        }
        const written = await this.#writeControl(this.#host.bluetoothDialerHello());
        if (written.tag !== "Written") {
            return sessionFailureToConnectFailure("bluetooth", "Handshake", written);
        }
        const confirmed = await this.#waitForPeer();
        if (confirmed.tag !== "Confirmed") {
            return confirmed;
        }
        void this.#outboundLoop();
        return Tag("Started");
    }
    async close() {
        if (this.#closed) {
            return closedSessionOutcome(this.#status);
        }
        this.#closed = true;
        const causes = [];
        if (this.#interfaceId) {
            const detached = this.#host.deactivateInterface(this.#interfaceId);
            if (detached.tag !== "Detached") {
                causes.push(Tag("RuntimeDetachFailed", { detail: detached.data.detail }));
            }
        }
        const pendingWrite = await this.#writeQueue;
        if (pendingWrite.tag !== "Written") {
            causes.push(Tag("TransportCloseFailed", {
                detail: describeInterfaceSessionFailure(pendingWrite),
            }));
        }
        const disconnected = disconnectBluetoothServer(this.#server);
        if (disconnected) {
            causes.push(disconnected);
        }
        if (hasCleanupFailures(causes)) {
            const failed = closeFailed(causes);
            this.#status = Tag("Failed", failed);
            return failed;
        }
        this.#status = Tag("Closed");
        return Tag("Closed");
    }
    async #waitForPeer() {
        const started = Date.now();
        while (!this.#confirmed && !this.#closed && !this.#connectFailure) {
            if (Date.now() - started > BLUETOOTH_HANDSHAKE_TIMEOUT_MS) {
                const timedOut = Tag("TimedOut", {
                    interface: "bluetooth",
                    stage: "Handshake",
                    timeoutMs: BLUETOOTH_HANDSHAKE_TIMEOUT_MS,
                });
                this.#abortConnect(timedOut);
                return timedOut;
            }
            await delay(25);
        }
        if (this.#connectFailure) {
            return this.#connectFailure;
        }
        if (!this.#confirmed) {
            return Tag("ConnectionFailed", {
                interface: "bluetooth",
                stage: "Handshake",
                detail: "Bluetooth link closed before peer confirmation",
            });
        }
        return Tag("Confirmed");
    }
    #handleControlEvent(event) {
        const decoded = characteristicBytes(event);
        if (decoded.tag !== "Decoded") {
            return decoded;
        }
        const bytes = decoded.data;
        let control;
        try {
            control = parseBluetoothControl(this.#host.bluetoothDecodeControl(bytes));
        }
        catch (error) {
            return Tag("ProtocolViolation", {
                protocol: "Bluetooth",
                detail: describeHostError(error),
            });
        }
        return match_into().from(control, {
            Hello: () => this.#data === this.#control
                ? this.#handleDataBytes(bytes)
                : Tag("Handled"),
            Welcome: (identity) => {
                if (this.#confirmed) {
                    return Tag("Handled");
                }
                let registration;
                try {
                    registration = {
                        interfaceName: "bluetooth",
                        supervisorKind: "bluetooth-auto",
                        kind: "bluetooth-peer",
                        channelTag: channelTag(identity),
                        bitrateBps: this.#host.bluetoothBitrateBps(),
                        hardwareMtu: this.#host.bluetoothHardwareMtu(),
                    };
                }
                catch (error) {
                    return Tag("ProtocolViolation", {
                        protocol: "Bluetooth",
                        detail: describeHostError(error),
                    });
                }
                const registered = this.#host.registerInterface(registration);
                if (registered.tag !== "Registered") {
                    return registered;
                }
                this.#interfaceId = registered.data;
                this.#confirmed = true;
                this.#status = Tag("Active");
                return Tag("Handled");
            },
            Close: () => {
                void this.close();
                return Tag("Handled");
            },
        });
    }
    #handleDataEvent(event) {
        const decoded = characteristicBytes(event);
        return decoded.tag === "Decoded"
            ? this.#handleDataBytes(decoded.data)
            : decoded;
    }
    #handleDataBytes(bytes) {
        if (!this.#confirmed || !this.#interfaceId) {
            return Tag("Handled");
        }
        let frame;
        try {
            frame = this.#reassembler.absorb(bytes);
        }
        catch (error) {
            return Tag("ProtocolViolation", {
                protocol: "Bluetooth",
                detail: describeHostError(error),
            });
        }
        if (frame && frame.length > 0) {
            const ingested = this.#host.ingest(this.#interfaceId, packetFrame(frame));
            return ingested.tag === "Accepted" ? Tag("Handled") : ingested;
        }
        return Tag("Handled");
    }
    #handleEventFailure(failure) {
        if (!this.#confirmed) {
            this.#abortConnect(failure.tag === "AlreadyActive"
                ? failure
                : sessionFailureToConnectFailure("bluetooth", "Handshake", failure));
            return;
        }
        const sessionFailure = failure.tag === "AlreadyActive"
            ? unexpectedSessionFailure(`Bluetooth peer became active more than once for ${failure.data.target}`)
            : failure;
        void this.#fail(sessionFailure);
    }
    #abortConnect(failure) {
        if (this.#closed) {
            return;
        }
        this.#connectFailure = failure;
        this.#status = Tag("Failed", failure.tag === "RuntimeRejected"
            ? failure
            : unexpectedSessionFailure(describeBluetoothConnectFailure(failure)));
        this.#closed = true;
        if (this.#interfaceId) {
            this.#host.deactivateInterface(this.#interfaceId);
        }
        disconnectBluetoothServer(this.#server);
    }
    async #outboundLoop() {
        try {
            while (!this.#closed) {
                const interfaceId = this.#interfaceId;
                if (this.#confirmed && interfaceId) {
                    const outbound = this.#host.takeOutboundFor(interfaceId);
                    if (outbound.tag !== "Outbound") {
                        await this.#fail(outbound);
                        return;
                    }
                    for (const frame of outbound.data) {
                        for (const fragment of this.#host.bluetoothDataFragments(frame.bytes)) {
                            const written = await this.#writeData(fragment);
                            if (written.tag !== "Written") {
                                await this.#fail(written);
                                return;
                            }
                        }
                    }
                }
                await delay(BLUETOOTH_OUTBOUND_POLL_MS);
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
        if (this.#interfaceId) {
            this.#host.deactivateInterface(this.#interfaceId);
        }
        await this.#writeQueue;
        disconnectBluetoothServer(this.#server);
    }
    async #writeControl(bytes) {
        return this.#write(this.#control, bytes);
    }
    async #writeData(bytes) {
        return this.#write(this.#data, bytes);
    }
    async #write(characteristic, bytes) {
        if (this.#closed || bytes.length === 0) {
            return Tag("Written");
        }
        const write = this.#writeQueue
            .then(async (previous) => {
            if (previous.tag !== "Written" || this.#closed) {
                return previous;
            }
            return writeBluetoothValue(characteristic, bytes);
        })
            .catch((error) => unexpectedSessionFailure(error));
        this.#writeQueue = write;
        return write;
    }
}
export class Prns {
    interfaces;
    #runtime;
    #entropy;
    #now;
    #limits;
    #resourceCompressionModuleUrl;
    #events;
    #diagnostics;
    #pendingCommands = new Map();
    #responseParts = new Map();
    #lifecycle = Tag("Running");
    constructor(wasm, runtime, entropy, now, bleIdentityAvailability, limits, resourceCompressionModuleUrl) {
        this.#runtime = runtime;
        this.#entropy = entropy;
        this.#now = now;
        this.#limits = limits;
        this.#resourceCompressionModuleUrl =
            resourceCompressionModuleUrl.href;
        this.#events = new BoundedAsyncLane({
            name: "ApplicationEvents",
            maximumValues: limits.applicationEvents,
            maximumBytes: limits.retainedEventBytes,
            measure: retainedBrowserEventBytes,
            onRejected: (rejectedEventBytes) => this.#failBackpressure(rejectedEventBytes),
            onBeforeNext: () => this.#pumpEvents(),
        });
        this.#diagnostics = new BoundedAsyncLane({
            name: "Diagnostics",
            maximumValues: limits.diagnostics,
            maximumBytes: Number.MAX_SAFE_INTEGER,
            measure: () => 0,
            gap: (count) => Tag("DiagnosticsDropped", { count }),
            onBeforeNext: () => this.#pumpEvents(),
        });
        this.interfaces = new PrnsInterfaces(new RuntimeHost(wasm, runtime, entropy, now, bleIdentityAvailability, () => this.#pumpEvents()));
    }
    static async create(options) {
        const loaded = options.wasm
            ? Tag("Loaded", options.wasm)
            : await loadBundledWasm();
        if (loaded.tag !== "Loaded") {
            return loaded;
        }
        const wasm = loaded.data;
        let actualAbi;
        let actualProductVersion;
        try {
            actualAbi = wasm.hostContractAbi();
            actualProductVersion = wasm.productVersion();
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
        if (actualAbi !== HOST_CONTRACT_ABI ||
            actualProductVersion !== PRODUCT_VERSION) {
            return Tag("ContractMismatch", {
                requiredAbi: HOST_CONTRACT_ABI,
                actualAbi,
                requiredProductVersion: PRODUCT_VERSION,
                actualProductVersion,
            });
        }
        let identityLength;
        try {
            identityLength = positiveInteger(wasm.identitySecretKeyLength(), "identity secret key length");
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
        const store = options.identityStore;
        let identity;
        if (store) {
            let loaded;
            try {
                loaded = await store.load(identityLength);
            }
            catch (error) {
                return Tag("IdentityStoreFailed", {
                    operation: "Load",
                    detail: describeHostError(error),
                });
            }
            if (loaded.tag === "Loaded") {
                try {
                    identity = identitySecretKey(loaded.data, identityLength);
                }
                catch (error) {
                    return Tag("StoredIdentityInvalid", {
                        detail: describeHostError(error),
                    });
                }
            }
            else if (loaded.tag !== "Missing") {
                return loaded;
            }
        }
        if (!identity) {
            const generated = webCryptoIdentity(identityLength);
            if (generated.tag !== "Generated") {
                return generated;
            }
            identity = generated.data;
            if (store) {
                let saved;
                try {
                    saved = await store.save(identity);
                }
                catch (error) {
                    return Tag("IdentityStoreFailed", {
                        operation: "Save",
                        detail: describeHostError(error),
                    });
                }
                if (saved.tag !== "Saved") {
                    return saved;
                }
            }
        }
        const bleIdentityAvailability = await loadOrCreateBleIdentity(options.bleIdentityStore ?? new BrowserLocalStorageBleIdentityStore());
        const bleIdentity = bleIdentityAvailability.tag === "Available"
            ? bleIdentityAvailability.data
            : undefined;
        try {
            const limits = browserLimits(options.limits ?? balancedLimits());
            return Tag("Ready", new Prns(wasm, new wasm.PrnsRuntime(identity, bleIdentity), options.entropy ?? webCryptoEntropy, options.now ?? nowMillis, bleIdentityAvailability, limits, options.resourceCompressionModuleUrl ??
                bundledWasmModuleUrl()));
        }
        catch (error) {
            return runtimeRejected("initialize", error);
        }
    }
    registerSingleDestination(options) {
        try {
            return Tag("Registered", destinationHash(this.#runtime.registerSingleDestination(options)));
        }
        catch (error) {
            return runtimeRejected("register-destination", error);
        }
    }
    registerNodePage(appData) {
        try {
            return Tag("Registered", destinationHash(this.#runtime.registerNodePage({ appData })));
        }
        catch (error) {
            return runtimeRejected("register-node-page", error);
        }
    }
    execute(command) {
        return this.#execute(command);
    }
    #execute(command) {
        if (this.#lifecycle.tag !== "Running") {
            return Promise.resolve(commandFailed(Tag("NodeStopped")));
        }
        return match_into().from(command, {
            Announce: ({ destination, interface: interfaceId }) => this.#issueCommand("announce", command, (entropy) => this.#runtime.announce({
                destination,
                ...(interfaceId === undefined ? {} : { interfaceId }),
                nowMs: this.#now(),
                entropy,
            })),
            SendSinglePacket: ({ destination, payload }) => this.#issueCommand("send-single-packet", command, (entropy) => this.#runtime.sendSinglePacket({
                destination,
                payload,
                nowMs: this.#now(),
                entropy,
            })),
            CloseLink: ({ linkId: value }) => this.#issueCommand("close-link", command, (entropy) => this.#runtime.closeLink({
                linkId: value,
                nowMs: this.#now(),
                entropy,
            })),
            AttachTcpServer: async () => commandFailed(Tag("UnsupportedByBackend")),
            AttachTcpClient: async () => commandFailed(Tag("UnsupportedByBackend")),
            AttachUdp: async () => commandFailed(Tag("UnsupportedByBackend")),
            DetachInterface: async () => commandFailed(Tag("UnsupportedByBackend")),
            EstablishLink: ({ destination }) => this.#issueCommand("establish-link", command, (entropy) => this.#runtime.establishLink({
                destination,
                nowMs: this.#now(),
                entropy,
            })),
            RequestPath: ({ destination }) => this.#issueCommand("request-path", command, (entropy) => this.#runtime.requestPath({
                destination,
                nowMs: this.#now(),
                entropy,
            })),
            Identify: ({ linkId: value, identity }) => this.#issueCommand("identify", command, (entropy) => this.#runtime.identify({
                linkId: value,
                identity,
                nowMs: this.#now(),
                entropy,
            })),
            SendLinkPacket: ({ linkId: value, payload }) => this.#issueCommand("send-link-packet", command, (entropy) => this.#runtime.sendLinkPacket({
                linkId: value,
                payload,
                nowMs: this.#now(),
                entropy,
            })),
            Request: ({ linkId: value, pathHash, payload, timeout }) => this.#issueCommand("request", command, (entropy) => this.#runtime.request({
                linkId: value,
                pathHash,
                payload,
                nowMs: this.#now(),
                entropy,
                ...runtimeResponseTimeout(timeout),
            })),
            Respond: ({ linkId: value, requestId: responseRequestId, requestRttMillis, payload, }) => this.#issueCommand("respond", command, (entropy) => this.#runtime.respond({
                linkId: value,
                requestId: responseRequestId,
                requestRttMillis,
                payload,
                nowMs: this.#now(),
                entropy,
            })),
            SendResource: ({ linkId: value, payload, packedMetadata, compression, }) => this.#sendResourceSource(value, byteResourceSource(payload), compression, packedMetadata),
            SetLinkResourceStrategy: ({ linkId: value, strategy }) => this.#issueCommand("set-link-resource-strategy", command, (entropy) => this.#runtime.setLinkResourceStrategy({
                linkId: value,
                nowMs: this.#now(),
                entropy,
                ...runtimeResourceStrategy(strategy),
            })),
            SetDestinationResourceStrategy: async ({ destination, strategy, }) => {
                try {
                    const configured = this.#runtime.setDestinationResourceStrategy({
                        destination,
                        ...runtimeResourceStrategy(strategy),
                    });
                    return configured
                        ? Tag("Succeeded", Tag("ResourceStrategySet"))
                        : commandFailed(Tag("UnknownDestination"));
                }
                catch (error) {
                    return commandFailed(browserCommandFailure("set-destination-resource-strategy", error));
                }
            },
            SendChannelMessage: ({ linkId: value, messageType, payload, }) => {
                if (!Number.isSafeInteger(messageType) ||
                    messageType < 0 ||
                    messageType > 0xefff) {
                    return Promise.resolve(commandFailed(Tag("InvalidChannelMessageType")));
                }
                return this.#issueCommand("send-channel-message", command, (entropy) => this.#runtime.sendChannelMessage({
                    linkId: value,
                    messageType,
                    payload,
                    nowMs: this.#now(),
                    entropy,
                }));
            },
            AllowRequester: ({ destination, pathHash, identity }) => this.#issueCommand("allow-requester", command, (entropy) => this.#runtime.allowRequester({
                destination,
                pathHash,
                identity,
                nowMs: this.#now(),
                entropy,
            })),
        });
    }
    announce(destination, interfaceId) {
        return this.execute(Tag("Announce", interfaceId === undefined
            ? { destination }
            : { destination, interface: interfaceId }));
    }
    sendSinglePacket(destination, payload) {
        return this.execute(Tag("SendSinglePacket", { destination, payload }));
    }
    closeLink(value) {
        return this.execute(Tag("CloseLink", { linkId: value }));
    }
    establishLink(destination) {
        return this.execute(Tag("EstablishLink", { destination }));
    }
    requestPath(destination) {
        return this.execute(Tag("RequestPath", { destination }));
    }
    identify(value, identity) {
        return this.execute(Tag("Identify", { linkId: value, identity }));
    }
    sendLinkPacket(value, payload) {
        return this.execute(Tag("SendLinkPacket", { linkId: value, payload }));
    }
    request(value, pathHash, payload, timeout = Tag("LinkDefault")) {
        return this.execute(Tag("Request", {
            linkId: value,
            pathHash,
            payload,
            timeout,
        }));
    }
    respond(value, responseRequestId, requestRttMillis, payload) {
        return this.execute(Tag("Respond", {
            linkId: value,
            requestId: responseRequestId,
            requestRttMillis,
            payload,
        }));
    }
    sendResource(value, payload, options = {}) {
        return this.execute(Tag("SendResource", {
            linkId: value,
            payload,
            compression: options.compression ?? Tag("Auto"),
            ...(options.packedMetadata === undefined
                ? {}
                : { packedMetadata: options.packedMetadata }),
        }));
    }
    sendResourceBlob(value, blob, options = {}) {
        return this.#sendResourceSource(value, blobResourceSource(blob), options.compression ?? Tag("Auto"), options.packedMetadata);
    }
    setLinkResourceStrategy(value, strategy) {
        return this.execute(Tag("SetLinkResourceStrategy", { linkId: value, strategy }));
    }
    setDestinationResourceStrategy(destination, strategy) {
        return this.execute(Tag("SetDestinationResourceStrategy", {
            destination,
            strategy,
        }));
    }
    sendChannelMessage(value, messageType, payload) {
        return this.execute(Tag("SendChannelMessage", {
            linkId: value,
            messageType,
            payload,
        }));
    }
    allowRequester(destination, pathHash, identity) {
        return this.execute(Tag("AllowRequester", { destination, pathHash, identity }));
    }
    get lifecycle() {
        return this.#lifecycle;
    }
    claimEvents() {
        this.#pumpEvents();
        return this.#events.claim();
    }
    claimDiagnostics() {
        this.#pumpEvents();
        return this.#diagnostics.claim();
    }
    snapshot() {
        try {
            return Tag("Captured", parseSnapshot(this.#runtime.snapshot()));
        }
        catch (error) {
            return runtimeRejected("snapshot", error);
        }
    }
    #entropyBytes() {
        return fillEntropy(this.#entropy, MIN_ENTROPY_BYTES);
    }
    #issueCommand(operation, command, issue) {
        return this.#issuePendingCommand(operation, Tag("HostCommand", { command }), issue);
    }
    #issueResourceSegment(input) {
        return this.#issuePendingCommand("send-resource", Tag("ResourceSegment"), (entropy) => this.#runtime.sendResourceSegment({
            ...input,
            nowMs: this.#now(),
            entropy,
        }));
    }
    #issuePendingCommand(operation, pending, issue) {
        if (this.#lifecycle.tag !== "Running") {
            return Promise.resolve(commandFailed(Tag("NodeStopped")));
        }
        if (this.#pendingCommands.size >= this.#limits.pendingCommands) {
            return Promise.resolve(commandFailed(Tag("Busy")));
        }
        const entropy = this.#entropyBytes();
        if (entropy.tag !== "Filled") {
            return Promise.resolve(commandFailed(Tag("EntropyUnavailable")));
        }
        let id;
        try {
            id = commandId(issue(entropy.data));
        }
        catch (error) {
            return Promise.resolve(commandFailed(browserCommandFailure(operation, error)));
        }
        return new Promise((settle) => {
            this.#pendingCommands.set(id, { pending, settle });
            this.#pumpEvents();
        });
    }
    #sendResourceSource(value, source, compression, packedMetadata) {
        if (this.#lifecycle.tag !== "Running") {
            return Promise.resolve(Tag("Failed", Tag("NodeStopped")));
        }
        return sendResourceFromSource(value, source, compression, packedMetadata, {
            maximumInFlightSegments: this.#limits.pendingCommands,
            plan: (input) => this.#runtime.resourceSegmentPlan(input),
            compress: (payload, metadata) => browserResourceCompressor.compress(payload, metadata, this.#resourceCompressionModuleUrl),
            issue: (input) => this.#issueResourceSegment(input),
        });
    }
    #pumpEvents() {
        if (this.#lifecycle.tag === "Failed" || this.#lifecycle.tag === "Stopped") {
            return;
        }
        let parsed;
        try {
            parsed = this.#runtime.drainEvents().map(parseEvent);
        }
        catch (error) {
            this.#failContract(describeHostError(error));
            return;
        }
        for (const event of parsed) {
            match(event, {
                Application: (application) => {
                    this.#events.push(application);
                },
                Diagnostic: (diagnostic) => {
                    this.#diagnostics.push(diagnostic);
                },
                CommandResponse: ({ commandId: responseCommandId, event }) => {
                    this.#events.push(event);
                    this.#responseParts.set(responseCommandId, [event.data.data]);
                },
                CommandResponseSegment: ({ commandId: responseCommandId, event, }) => {
                    this.#events.push(event);
                    const parts = this.#responseParts.get(responseCommandId) ?? [];
                    parts.push(event.data.data);
                    this.#responseParts.set(responseCommandId, parts);
                },
                CommandSettled: ({ commandId, settlement }) => {
                    if (settlement === undefined) {
                        return;
                    }
                    const pending = this.#pendingCommands.get(commandId);
                    if (pending === undefined) {
                        return;
                    }
                    this.#pendingCommands.delete(commandId);
                    pending.settle(match(pending.pending, {
                        HostCommand: ({ command }) => this.#commandSettlement(commandId, command, settlement),
                        ResourceSegment: () => settlement,
                    }));
                },
            });
        }
    }
    #commandSettlement(id, command, settlement) {
        if (settlement.tag === "Failed") {
            this.#responseParts.delete(id);
            return settlement;
        }
        if (command.tag === "Request") {
            if (settlement.data.tag !== "PacketDelivered") {
                this.#responseParts.delete(id);
                return commandFailed(Tag("WriteFailed", {
                    detail: "request settled without delivery evidence",
                }));
            }
            const parts = this.#responseParts.get(id);
            this.#responseParts.delete(id);
            if (parts === undefined) {
                return commandFailed(Tag("WriteFailed", {
                    detail: "request settled without response data",
                }));
            }
            return Tag("Succeeded", Tag("ResponseReceived", {
                data: concatenateBytes(parts),
                rttMillis: settlement.data.data.rttMillis,
            }));
        }
        if (command.tag === "Respond") {
            if (settlement.data.tag !== "ResponseSent") {
                return commandFailed(Tag("WriteFailed", {
                    detail: "response settled with an unexpected outcome",
                }));
            }
            return Tag("Succeeded", Tag("ResponseSent", {
                rttMillis: command.data.requestRttMillis,
            }));
        }
        return settlement;
    }
    #failBackpressure(rejectedEventBytes) {
        this.#lifecycle = Tag("Failed", {
            cause: "EventBackpressureExceeded",
            limits: this.#limits,
            rejectedEventBytes,
        });
        this.#events.finish();
        this.#diagnostics.finish();
        this.#settleFailedCommands("application event backpressure exceeded");
    }
    #failContract(detail) {
        this.#lifecycle = Tag("Failed", {
            cause: "ContractViolated",
            detail,
        });
        const error = new Error(detail);
        this.#events.fail(error);
        this.#diagnostics.fail(error);
        this.#settleFailedCommands(detail);
    }
    #settleFailedCommands(detail) {
        for (const pending of this.#pendingCommands.values()) {
            pending.settle(commandFailed(Tag("WriteFailed", { detail })));
        }
        this.#pendingCommands.clear();
        this.#responseParts.clear();
    }
}
class RuntimeHost {
    #wasm;
    #runtime;
    #entropy;
    #now;
    #bleIdentityAvailability;
    #onRuntimeActivity;
    #activeInterfaces = new Map();
    #activeRegistrationKeys = new Set();
    #outboundQueues = new Map();
    #overflowedOutbound = new Set();
    constructor(wasm, runtime, entropy, now, bleIdentityAvailability, onRuntimeActivity) {
        this.#wasm = wasm;
        this.#runtime = runtime;
        this.#entropy = entropy;
        this.#now = now;
        this.#bleIdentityAvailability = bleIdentityAvailability;
        this.#onRuntimeActivity = onRuntimeActivity;
    }
    runtimeReadiness() {
        try {
            this.#runtime.snapshot();
            return Tag("Ready");
        }
        catch (error) {
            return runtimeRejected("inspect-readiness", error);
        }
    }
    registerInterface(registration) {
        const { interfaceName, supervisorKind = registration.kind, ...options } = registration;
        const registrationKey = `${options.kind}:${byteKey(options.channelTag)}`;
        if (this.#activeRegistrationKeys.has(registrationKey)) {
            return Tag("AlreadyActive", {
                interface: interfaceName,
                target: registrationKey,
            });
        }
        let id;
        try {
            id = interfaceId(this.#runtime.registerInterface({ ...options, nowMs: this.#now() }));
        }
        catch (error) {
            return runtimeRejected("register-interface", error);
        }
        const key = byteKey(id);
        if (this.#activeInterfaces.has(key)) {
            return Tag("AlreadyActive", {
                interface: interfaceName,
                target: key,
            });
        }
        this.#activeRegistrationKeys.add(registrationKey);
        this.#activeInterfaces.set(key, { id, registrationKey, supervisorKind });
        this.#outboundQueues.set(key, []);
        return Tag("Registered", id);
    }
    deactivateInterface(id) {
        const key = byteKey(id);
        const active = this.#activeInterfaces.get(key);
        if (!active) {
            return Tag("Detached");
        }
        try {
            const removed = this.#runtime.removeInterface({
                interfaceId: id,
                nowMs: this.#now(),
            });
            if (!removed) {
                return runtimeRejected("remove-interface", `runtime did not contain interface ${key}`);
            }
        }
        catch (error) {
            return runtimeRejected("remove-interface", error);
        }
        this.#activeInterfaces.delete(key);
        this.#activeRegistrationKeys.delete(active.registrationKey);
        this.#outboundQueues.delete(key);
        this.#overflowedOutbound.delete(key);
        return Tag("Detached");
    }
    ingest(interfaceId, bytes) {
        const entropy = this.entropy();
        if (entropy.tag !== "Filled") {
            return entropy;
        }
        try {
            this.#runtime.ingest({
                interfaceId,
                bytes,
                nowMs: this.#now(),
                entropy: entropy.data,
            });
            this.#onRuntimeActivity();
            return Tag("Accepted");
        }
        catch (error) {
            return runtimeRejected("ingest", error);
        }
    }
    drainOutbound() {
        try {
            return Tag("Drained", this.#runtime.drainOutbound().map(parseOutboundFrame));
        }
        catch (error) {
            return runtimeRejected("drain-outbound", error);
        }
    }
    takeOutboundFor(interfaceId) {
        const interfaceKey = byteKey(interfaceId);
        const direct = [];
        const drained = this.drainOutbound();
        if (drained.tag !== "Drained") {
            return drained;
        }
        for (const frame of drained.data) {
            for (const [key, active] of this.#activeInterfaces) {
                if (outboundTargets(frame.target, active.id, active.supervisorKind)) {
                    if (key === interfaceKey) {
                        direct.push(frame);
                        continue;
                    }
                    const queue = this.#outboundQueues.get(key);
                    if (queue && queue.length < INTERFACE_OUTBOUND_QUEUE_DEPTH) {
                        queue.push(frame);
                    }
                    else if (queue) {
                        this.#overflowedOutbound.add(key);
                    }
                }
            }
        }
        if (this.#overflowedOutbound.delete(interfaceKey)) {
            this.#outboundQueues.set(interfaceKey, []);
            return Tag("OutboundQueueFull", {
                capacity: INTERFACE_OUTBOUND_QUEUE_DEPTH,
            });
        }
        const queued = this.#outboundQueues.get(interfaceKey) ?? [];
        this.#outboundQueues.set(interfaceKey, []);
        return Tag("Outbound", queued.concat(direct));
    }
    createUsbAutoDecoder() {
        return new this.#wasm.UsbAutoDecoder();
    }
    createBluetoothReassembler() {
        return new this.#wasm.BluetoothReassembler();
    }
    bluetoothServiceUuid() {
        return this.#wasm.bluetoothServiceUuid();
    }
    bluetoothIdentityReadiness() {
        return this.#bleIdentityAvailability.tag === "Available"
            ? Tag("Ready")
            : this.#bleIdentityAvailability;
    }
    bluetoothControlUuid() {
        return this.#wasm.bluetoothControlUuid();
    }
    bluetoothDataUuid() {
        return this.#wasm.bluetoothDataUuid();
    }
    bluetoothBitrateBps() {
        return bitrateBps(this.#wasm.bluetoothBitrateBps());
    }
    bluetoothHardwareMtu() {
        return hardwareMtu(this.#wasm.bluetoothHardwareMtu());
    }
    bluetoothDialerHello() {
        return this.#wasm.bluetoothDialerHello(this.#runtime.bluetoothIdentity());
    }
    bluetoothDecodeControl(bytes) {
        return this.#wasm.bluetoothDecodeControl(bytes);
    }
    bluetoothDataFragments(packet) {
        return this.#wasm.bluetoothDataFragments(packet);
    }
    websocketBitrateBps() {
        return bitrateBps(this.#wasm.websocketBitrateBps());
    }
    websocketFrameCap() {
        return positiveInteger(this.#wasm.websocketFrameCap(), "WebSocket frame cap");
    }
    websocketHardwareMtu() {
        return hardwareMtu(this.#wasm.websocketHardwareMtu());
    }
    autoWifiReady() {
        return this.runtimeReadiness();
    }
    autoWifiRegister(id) {
        try {
            return this.registerInterface({
                interfaceName: "auto-wifi",
                kind: "auto-wifi",
                channelTag: channelTag(id),
                bitrateBps: this.websocketBitrateBps(),
                hardwareMtu: this.websocketHardwareMtu(),
            });
        }
        catch (error) {
            return runtimeRejected("register-interface", error);
        }
    }
    autoWifiDeactivate(id) {
        return this.deactivateInterface(id);
    }
    autoWifiIngest(id, bytes) {
        try {
            return this.ingest(id, packetFrame(bytes));
        }
        catch (error) {
            return runtimeRejected("ingest", error);
        }
    }
    autoWifiTakeOutbound(id) {
        return this.takeOutboundFor(id);
    }
    autoWifiBitrateBps() {
        return this.websocketBitrateBps();
    }
    autoWifiHardwareMtu() {
        return this.websocketHardwareMtu();
    }
    autoWifiFrameCap() {
        return this.websocketFrameCap();
    }
    usbAutoHostBitrateBps() {
        return bitrateBps(this.#wasm.usbAutoHostBitrateBps());
    }
    usbAutoHostHardwareMtu() {
        return hardwareMtu(this.#wasm.usbAutoHostHardwareMtu());
    }
    defaultUsbAutoFilters() {
        return [
            {
                vendorId: this.#wasm.usbAutoWebUsbVendorId(),
                productId: this.#wasm.usbAutoWebUsbProductId(),
            },
        ];
    }
    usbAutoNodeTagFor(interfaceId) {
        return this.#wasm.usbAutoNodeTagFor(interfaceId);
    }
    usbAutoHostHelloFrame() {
        return this.#wasm.usbAutoHostHelloFrame();
    }
    usbAutoHostHelloAckFrame(nodeTag) {
        return this.#wasm.usbAutoHostHelloAckFrame(nodeTag);
    }
    usbAutoDataFrame(packet) {
        return this.#wasm.usbAutoDataFrame(packet);
    }
    entropy() {
        return fillEntropy(this.#entropy, MIN_ENTROPY_BYTES);
    }
}
export function identitySecretKey(bytes, expectedLength) {
    return exactBytes(bytes, expectedLength, "IdentitySecretKey");
}
export function bleIdentity(bytes) {
    return bytes.length === BLE_IDENTITY_LENGTH
        ? Tag("ValidBleIdentity", copyBytes(bytes))
        : Tag("InvalidBleIdentity", { actualLength: bytes.length });
}
export function channelTag(bytes) {
    return nonEmptyBytes(bytes, "ChannelTag");
}
export function packetFrame(bytes) {
    return nonEmptyBytes(bytes, "PacketFrame");
}
export function entropyBytes(bytes) {
    if (bytes.length < MIN_ENTROPY_BYTES) {
        throw new PrnsValidationError("invalid-length", `EntropyBytes requires at least ${MIN_ENTROPY_BYTES} bytes`);
    }
    return copyBytes(bytes);
}
export function appData(bytes = new Uint8Array()) {
    return copyBytes(bytes);
}
export function appName(value) {
    return dottedComponent(value, "AppName");
}
export function aspect(value) {
    return dottedComponent(value, "Aspect");
}
export function bitrateBps(value) {
    return positiveInteger(value, "BitrateBps");
}
export function hardwareMtu(value) {
    return positiveInteger(value, "HardwareMtu");
}
export function hopCount(value) {
    if (!Number.isInteger(value) || value < 0 || value > 255) {
        throw new PrnsValidationError("invalid-number", "HopCount must be an integer from 0 through 255");
    }
    return value;
}
export function nowMillis(value = Date.now()) {
    if (!Number.isSafeInteger(value) || value < 0) {
        throw new PrnsValidationError("invalid-number", "InstantMillis must be a non-negative safe integer");
    }
    return value;
}
export function commandId(value) {
    if (value < 0n) {
        throw new PrnsValidationError("invalid-number", "CommandId must be non-negative");
    }
    return value;
}
export function webCryptoEntropy(length) {
    try {
        if (!hostGlobal().crypto) {
            return Tag("HostApiUnavailable", { api: "Crypto" });
        }
        const bytes = webCryptoBytes(length);
        if (bytes.length < MIN_ENTROPY_BYTES) {
            return Tag("InsufficientEntropy", {
                minimum: MIN_ENTROPY_BYTES,
                actual: bytes.length,
            });
        }
        return Tag("Filled", bytes);
    }
    catch (error) {
        return Tag("EntropySourceFailed", { detail: describeHostError(error) });
    }
}
function outboundTargets(target, interfaceId, supervisorKind) {
    return match_into().from(target, {
        Interface: (targetInterface) => equalBytes(targetInterface, interfaceId),
        Broadcast: ({ supervisorKind: targetKind, fan }) => targetKind === supervisorKind &&
            match_into().from(fan, {
                All: () => true,
                Only: (targetInterface) => equalBytes(targetInterface, interfaceId),
                AllExcept: (targetInterface) => !equalBytes(targetInterface, interfaceId),
            }),
    });
}
const RAW_USB_AUTO_MESSAGE_TYPES = new Set(["hello", "helloAck", "data"]);
function parseUsbAutoMessage(raw) {
    const object = record(raw, "UsbAutoInboundMessage");
    const type = stringField(object, "type");
    if (!RAW_USB_AUTO_MESSAGE_TYPES.has(type)) {
        throw new PrnsValidationError("invalid-component", `unknown USB-auto message ${type}`);
    }
    return match(type, {
        hello: () => Tag("Hello"),
        helloAck: () => Tag("HelloAck", bytesField(object, "tag")),
        data: () => Tag("Data", bytesField(object, "bytes")),
    });
}
const RAW_BLUETOOTH_CONTROL_TYPES = new Set(["hello", "welcome", "close"]);
function parseBluetoothControl(raw) {
    const object = record(raw, "BluetoothControl");
    const type = stringField(object, "type");
    if (!RAW_BLUETOOTH_CONTROL_TYPES.has(type)) {
        throw new PrnsValidationError("invalid-component", `unknown Bluetooth control ${type}`);
    }
    return match(type, {
        hello: () => Tag("Hello", bytesField(object, "identity")),
        welcome: () => Tag("Welcome", bytesField(object, "identity")),
        close: () => Tag("Close", stringField(object, "reason")),
    });
}
function parseOutboundFrame(raw) {
    const object = record(raw, "PrnsOutboundFrame");
    const type = stringField(object, "type");
    if (type !== "frame" && type !== "announce") {
        throw new PrnsValidationError("unknown-outbound-target", `unknown outbound frame type ${type}`);
    }
    const frame = {
        type,
        target: parseOutboundTarget(field(object, "target")),
        bytes: packetFrame(bytesField(object, "bytes")),
    };
    const hops = optionalNumber(object, "hops", hopCount);
    if (hops !== undefined) {
        frame.hops = hops;
    }
    return frame;
}
function parseOutboundTarget(raw) {
    const object = record(raw, "OutboundTarget");
    const type = stringField(object, "type");
    if (type === "interface") {
        return Tag("Interface", interfaceId(bytesField(object, "interfaceId")));
    }
    if (type === "broadcast") {
        return Tag("Broadcast", {
            supervisorKind: parseRuntimeInterfaceKind(stringField(object, "supervisorKind")),
            fan: parseFanTarget(field(object, "fan")),
        });
    }
    throw new PrnsValidationError("unknown-outbound-target", `unknown outbound target ${type}`);
}
function parseFanTarget(raw) {
    const object = record(raw, "FanTarget");
    const type = stringField(object, "type");
    if (type === "all") {
        return Tag("All");
    }
    if (type === "only") {
        return Tag("Only", interfaceId(bytesField(object, "interfaceId")));
    }
    if (type === "allExcept") {
        return Tag("AllExcept", interfaceId(bytesField(object, "interfaceId")));
    }
    throw new PrnsValidationError("unknown-outbound-target", `unknown fan target ${type}`);
}
function parseEvent(raw) {
    const object = record(raw, "PrnsEvent");
    const event = Tag(rawEventType(stringField(object, "type")), object);
    return match_into().from(event, {
        announce: (data) => Tag("Diagnostic", Tag("AnnounceHeard", {
            destination: destinationHash(bytesField(data, "destination")),
            hops: hopCount(numberField(data, "hops")),
            sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        })),
        selfRatchetRotated: (data) => Tag("Diagnostic", Tag("SelfRatchetRotated", {
            destination: destinationHash(bytesField(data, "destination")),
        })),
        announceHeldDropped: (data) => Tag("Diagnostic", Tag("AnnounceHeldDropped", {
            destination: destinationHash(bytesField(data, "destination")),
            sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
            cause: stringField(data, "cause"),
        })),
        commandSettled: (data) => {
            const commandIdValue = commandId(bigintField(data, "id"));
            const settlement = parseCommandSettlement(data);
            return Tag("CommandSettled", settlement === undefined
                ? { commandId: commandIdValue }
                : { commandId: commandIdValue, settlement });
        },
        linkEstablished: (data) => Tag("Diagnostic", Tag("LinkEstablished", {
            linkId: linkId(bytesField(data, "linkId")),
            rttMillis: nonNegativeInteger(numberField(data, "rttMillis"), "rttMillis"),
        })),
        peerIdentified: (data) => Tag("Diagnostic", Tag("PeerIdentified", {
            linkId: linkId(bytesField(data, "linkId")),
            identity: identityHash(bytesField(data, "identity")),
        })),
        request: (data) => {
            const request = {
                destination: destinationHash(bytesField(data, "destination")),
                linkId: linkId(bytesField(data, "linkId")),
                requestId: requestId(bytesField(data, "requestId")),
                pathHash: requestPathHash(bytesField(data, "pathHash")),
                rttMillis: nonNegativeInteger(numberField(data, "rttMillis"), "rttMillis"),
                data: copyBytes(bytesField(data, "data")),
            };
            const requester = optionalBytesField(data, "requester");
            return Tag("Application", Tag("Request", requester
                ? { ...request, requester: identityHash(requester) }
                : request));
        },
        response: (data) => {
            const responseCommandId = commandId(bigintField(data, "commandId"));
            return Tag("CommandResponse", {
                commandId: responseCommandId,
                event: Tag("Response", {
                    linkId: linkId(bytesField(data, "linkId")),
                    requestId: requestId(bytesField(data, "requestId")),
                    data: copyBytes(bytesField(data, "data")),
                }),
            });
        },
        responseSegment: (data) => {
            const responseCommandId = commandId(bigintField(data, "commandId"));
            return Tag("CommandResponseSegment", {
                commandId: responseCommandId,
                event: Tag("ResponseSegment", {
                    linkId: linkId(bytesField(data, "linkId")),
                    requestId: requestId(bytesField(data, "requestId")),
                    segmentIndex: nonNegativeInteger(numberField(data, "segmentIndex"), "segmentIndex"),
                    totalSegments: positiveInteger(numberField(data, "totalSegments"), "totalSegments"),
                    data: copyBytes(bytesField(data, "data")),
                }),
            });
        },
        channelMessage: (data) => Tag("Application", Tag("ChannelMessage", {
            linkId: linkId(bytesField(data, "linkId")),
            messageType: nonNegativeInteger(numberField(data, "messageType"), "messageType"),
            data: copyBytes(bytesField(data, "data")),
        })),
        singleDelivery: (data) => Tag("Application", Tag("SingleDelivery", {
            destination: destinationHash(bytesField(data, "destination")),
            plaintext: copyBytes(bytesField(data, "plaintext")),
            sourceInterface: interfaceId(bytesField(data, "sourceInterface")),
        })),
        delivered: (data) => Tag("Diagnostic", Tag("Delivered", { detail: stringField(data, "detail") })),
        linkClosed: (data) => Tag("Diagnostic", Tag("LinkClosed", {
            linkId: linkId(bytesField(data, "linkId")),
            reason: linkClosedReason(stringField(data, "reason")),
        })),
        linkInterfaceMismatch: (data) => Tag("Diagnostic", Tag("LinkInterfaceMismatch", {
            linkId: linkId(bytesField(data, "linkId")),
            attachedInterface: interfaceId(bytesField(data, "attachedInterface")),
            arrivedOn: interfaceId(bytesField(data, "arrivedOn")),
        })),
        resourceReceived: (data) => {
            const details = {
                linkId: linkId(bytesField(data, "linkId")),
                hash: resourceHash(bytesField(data, "hash")),
                resource: new MemoryResourceStream(bytesField(data, "data")),
            };
            const metadata = optionalBytesField(data, "metadata");
            return Tag("Application", Tag("ResourceAvailable", metadata
                ? { ...details, metadata: copyBytes(metadata) }
                : details));
        },
        resourceFailed: (data) => Tag("Diagnostic", Tag("ResourceFailed", {
            linkId: linkId(bytesField(data, "linkId")),
            hash: resourceHash(bytesField(data, "hash")),
            cause: stringField(data, "cause"),
        })),
        resourceNeedsDecompression: (data) => Tag("Application", Tag("ResourceNeedsDecompression", {
            linkId: linkId(bytesField(data, "linkId")),
            hash: resourceHash(bytesField(data, "hash")),
            stream: copyBytes(bytesField(data, "stream")),
            uncompressedDataBytes: nonNegativeInteger(numberField(data, "uncompressedDataBytes"), "uncompressedDataBytes"),
        })),
        resourceSegment: (data) => {
            const details = {
                linkId: linkId(bytesField(data, "linkId")),
                originalHash: resourceHash(bytesField(data, "originalHash")),
                segmentIndex: nonNegativeInteger(numberField(data, "segmentIndex"), "segmentIndex"),
                totalSegments: positiveInteger(numberField(data, "totalSegments"), "totalSegments"),
                data: copyBytes(bytesField(data, "data")),
            };
            const metadata = optionalBytesField(data, "metadata");
            return Tag("Application", Tag("ResourceSegment", metadata
                ? { ...details, metadata: copyBytes(metadata) }
                : details));
        },
        resourceAssembled: (data) => Tag("Diagnostic", Tag("ResourceAssembled", {
            linkId: linkId(bytesField(data, "linkId")),
            originalHash: resourceHash(bytesField(data, "originalHash")),
            totalSizeBytes: nonNegativeInteger(numberField(data, "totalSizeBytes"), "totalSizeBytes"),
        })),
        routeExpired: (data) => Tag("Diagnostic", Tag("RouteExpired", {
            destination: destinationHash(bytesField(data, "destination")),
        })),
        routeEvicted: (data) => Tag("Diagnostic", Tag("RouteEvicted", {
            destination: destinationHash(bytesField(data, "destination")),
        })),
        routeInterfaceGone: (data) => Tag("Diagnostic", Tag("RouteInterfaceGone", {
            destination: destinationHash(bytesField(data, "destination")),
        })),
        routeDropped: (data) => Tag("Diagnostic", Tag("RouteDropped", {
            destination: destinationHash(bytesField(data, "destination")),
        })),
    });
}
function parseCommandSettlement(value) {
    const result = stringField(value, "result");
    if (result === "untracked") {
        return undefined;
    }
    if (result === "failed") {
        return commandFailed(parseCommandFailure(value));
    }
    if (result !== "succeeded") {
        throw new PrnsValidationError("invalid-component", `unknown command settlement result ${result}`);
    }
    const kind = stringField(value, "kind");
    if (kind === "Announced") {
        return Tag("Succeeded", Tag("Announced"));
    }
    if (kind === "LinkCloseQueued") {
        return Tag("Succeeded", Tag("LinkCloseQueued"));
    }
    if (kind === "PacketDelivered") {
        const delivered = {
            rttMillis: nonNegativeInteger(numberField(value, "rttMillis"), "rttMillis"),
            evidence: parseDeliveryEvidence(stringField(value, "evidence")),
        };
        const hash = optionalBytesField(value, "packetHash");
        return Tag("Succeeded", Tag("PacketDelivered", hash === undefined
            ? delivered
            : { ...delivered, packetHash: packetHash(hash) }));
    }
    if (kind === "LinkEstablished") {
        return Tag("Succeeded", Tag("LinkEstablished", {
            linkId: linkId(bytesField(value, "linkId")),
            rttMillis: nonNegativeInteger(numberField(value, "rttMillis"), "rttMillis"),
        }));
    }
    if (kind === "PathDiscovered") {
        return Tag("Succeeded", Tag("PathDiscovered", {
            hops: nonNegativeInteger(numberField(value, "hops"), "hops"),
        }));
    }
    if (kind === "Identified") {
        return Tag("Succeeded", Tag("Identified"));
    }
    if (kind === "ResponseSent") {
        return Tag("Succeeded", Tag("ResponseSent", {
            rttMillis: nonNegativeInteger(numberField(value, "rttMillis"), "rttMillis"),
        }));
    }
    if (kind === "ResourceSent") {
        return Tag("Succeeded", Tag("ResourceSent"));
    }
    if (kind === "ResourceStrategySet") {
        return Tag("Succeeded", Tag("ResourceStrategySet"));
    }
    if (kind === "RequesterAllowed") {
        return Tag("Succeeded", Tag("RequesterAllowed"));
    }
    throw new PrnsValidationError("invalid-component", `unknown command outcome ${kind}`);
}
function parseCommandFailure(value) {
    const kind = stringField(value, "kind");
    if (kind === "NodeStopped") {
        return Tag("NodeStopped");
    }
    if (kind === "Busy") {
        return Tag("Busy");
    }
    if (kind === "PayloadTooLarge") {
        return Tag("PayloadTooLarge");
    }
    if (kind === "UnknownDestination") {
        return Tag("UnknownDestination");
    }
    if (kind === "NotSingleDestination") {
        return Tag("NotSingleDestination");
    }
    if (kind === "AnnounceAppDataTooLong") {
        return Tag("AnnounceAppDataTooLong");
    }
    if (kind === "UnknownInterface") {
        return Tag("UnknownInterface");
    }
    if (kind === "NoRouteToDestination") {
        return Tag("NoRouteToDestination");
    }
    if (kind === "NotDirectlyReachable") {
        return Tag("NotDirectlyReachable");
    }
    if (kind === "PacketCulled") {
        return Tag("PacketCulled");
    }
    if (kind === "DeliveryTimedOut") {
        return Tag("DeliveryTimedOut");
    }
    if (kind === "InvalidBitrate") {
        return Tag("InvalidBitrate");
    }
    if (kind === "BindFailed") {
        return Tag("BindFailed", { detail: stringField(value, "detail") });
    }
    if (kind === "WriteFailed") {
        return Tag("WriteFailed", { detail: stringField(value, "detail") });
    }
    if (kind === "UnsupportedByBackend") {
        return Tag("UnsupportedByBackend");
    }
    if (kind === "UnknownLink") {
        return Tag("UnknownLink");
    }
    if (kind === "LinkNotActive") {
        return Tag("LinkNotActive");
    }
    if (kind === "EntropyUnavailable") {
        return Tag("EntropyUnavailable");
    }
    if (kind === "NotLinkInitiator") {
        return Tag("NotLinkInitiator");
    }
    if (kind === "IdentityNotHeld") {
        return Tag("IdentityNotHeld");
    }
    if (kind === "UnknownRequestHandler") {
        return Tag("UnknownRequestHandler");
    }
    if (kind === "RequestPolicyNotAllowList") {
        return Tag("RequestPolicyNotAllowList");
    }
    if (kind === "RequestAllowListFull") {
        return Tag("RequestAllowListFull");
    }
    if (kind === "LinkBusy") {
        return Tag("LinkBusy");
    }
    if (kind === "ResourceTableFull") {
        return Tag("ResourceTableFull");
    }
    if (kind === "ResourceMetadataTooLarge") {
        return Tag("ResourceMetadataTooLarge");
    }
    if (kind === "ResourceRejectedByPeer") {
        return Tag("ResourceRejectedByPeer");
    }
    if (kind === "ResourceSequencingFailed") {
        return Tag("ResourceSequencingFailed");
    }
    if (kind === "ResourcePredecessorFailed") {
        return Tag("ResourcePredecessorFailed");
    }
    if (kind === "ChannelWindowFull") {
        return Tag("ChannelWindowFull");
    }
    if (kind === "ChannelUntrackable") {
        return Tag("ChannelUntrackable");
    }
    if (kind === "InvalidChannelMessageType") {
        return Tag("InvalidChannelMessageType");
    }
    throw new PrnsValidationError("invalid-component", `unknown command failure ${kind}`);
}
function parseDeliveryEvidence(value) {
    if (value === "ExplicitProof" ||
        value === "ImplicitProof" ||
        value === "Response") {
        return value;
    }
    throw new PrnsValidationError("invalid-component", `unknown delivery evidence ${value}`);
}
function parseSnapshot(raw) {
    const object = record(raw, "PrnsSnapshot");
    const interfacesRaw = field(object, "interfaces");
    if (!Array.isArray(interfacesRaw)) {
        throw new PrnsValidationError("invalid-component", "snapshot interfaces must be an array");
    }
    return {
        type: literalField(object, "type", "snapshot"),
        ingestedPackets: nonNegativeInteger(numberField(object, "ingestedPackets"), "ingestedPackets"),
        ingestedCommands: nonNegativeInteger(numberField(object, "ingestedCommands"), "ingestedCommands"),
        routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
        scheduledAnnounces: nonNegativeInteger(numberField(object, "scheduledAnnounces"), "scheduledAnnounces"),
        interfaces: interfacesRaw.map(parseInterfaceSnapshot),
    };
}
function parseInterfaceSnapshot(raw) {
    const object = record(raw, "InterfaceSnapshot");
    const snapshot = {
        id: interfaceId(bytesField(object, "id")),
        kind: stringField(object, "kind"),
        routes: nonNegativeInteger(numberField(object, "routes"), "routes"),
        links: nonNegativeInteger(numberField(object, "links"), "links"),
    };
    const bitrate = optionalNumber(object, "bitrateBps", bitrateBps);
    if (bitrate !== undefined) {
        snapshot.bitrateBps = bitrate;
    }
    const mtu = optionalNumber(object, "hardwareMtu", hardwareMtu);
    if (mtu !== undefined) {
        snapshot.hardwareMtu = mtu;
    }
    return snapshot;
}
function parseRuntimeInterfaceKind(value) {
    if (value === "auto-usb-host" ||
        value === "auto-usb-device" ||
        value === "rnode" ||
        value === "bluetooth-auto" ||
        value === "bluetooth-peer" ||
        value === "auto-wifi" ||
        value === "websocket-client" ||
        value === "websocket-server" ||
        value === "websocket-server-peer" ||
        value === "serial" ||
        value === "kiss" ||
        value === "pipe") {
        return value;
    }
    throw new PrnsValidationError("unknown-interface-kind", `unknown interface kind ${value}`);
}
function exactBytes(bytes, expectedLength, name) {
    if (bytes.length !== expectedLength) {
        throw new PrnsValidationError("invalid-length", `${name} must be ${expectedLength} bytes`);
    }
    return copyBytes(bytes);
}
function nonEmptyBytes(bytes, name) {
    if (bytes.length === 0) {
        throw new PrnsValidationError("empty-bytes", `${name} must not be empty`);
    }
    return copyBytes(bytes);
}
function copyBytes(bytes) {
    return new Uint8Array(bytes);
}
function dottedComponent(value, name) {
    if (value.length === 0) {
        throw new PrnsValidationError("empty-string", `${name} must not be empty`);
    }
    if (value.includes(".")) {
        throw new PrnsValidationError("invalid-component", `${name} must not contain dots`);
    }
    return value;
}
function positiveInteger(value, name) {
    if (!Number.isSafeInteger(value) || value <= 0) {
        throw new PrnsValidationError("invalid-number", `${name} must be a positive safe integer`);
    }
    return value;
}
function nonNegativeInteger(value, name) {
    if (!Number.isSafeInteger(value) || value < 0) {
        throw new PrnsValidationError("invalid-number", `${name} must be a non-negative safe integer`);
    }
    return value;
}
function field(object, key) {
    if (!(key in object)) {
        throw new PrnsValidationError("invalid-component", `missing field ${key}`);
    }
    return object[key];
}
function stringField(object, key) {
    const value = field(object, key);
    if (typeof value !== "string") {
        throw new PrnsValidationError("invalid-component", `${key} must be a string`);
    }
    return value;
}
function literalField(object, key, expected) {
    const value = stringField(object, key);
    if (value !== expected) {
        throw new PrnsValidationError("invalid-component", `${key} must be ${expected}`);
    }
    return expected;
}
function numberField(object, key) {
    const value = field(object, key);
    if (typeof value !== "number") {
        throw new PrnsValidationError("invalid-component", `${key} must be a number`);
    }
    return value;
}
function optionalNumber(object, key, parse) {
    if (!(key in object)) {
        return undefined;
    }
    return parse(numberField(object, key));
}
function optionalBytesField(object, key) {
    return key in object ? bytesField(object, key) : undefined;
}
function bigintField(object, key) {
    const value = field(object, key);
    if (typeof value === "bigint") {
        return value;
    }
    if (typeof value === "number" && Number.isSafeInteger(value)) {
        return BigInt(value);
    }
    throw new PrnsValidationError("invalid-component", `${key} must be a bigint or safe integer`);
}
function bytesField(object, key) {
    const value = field(object, key);
    if (!(value instanceof Uint8Array)) {
        throw new PrnsValidationError("invalid-component", `${key} must be a Uint8Array`);
    }
    return value;
}
function record(value, name) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new PrnsValidationError("invalid-component", `${name} must be an object`);
    }
    return value;
}
function webCryptoBytes(length) {
    if (!Number.isSafeInteger(length) || length <= 0) {
        throw new PrnsValidationError("invalid-number", "random byte length must be a positive safe integer");
    }
    const out = new Uint8Array(length);
    const crypto = hostGlobal().crypto;
    if (!crypto) {
        throw new PrnsValidationError("missing-host-api", "Prns entropy requires globalThis.crypto.getRandomValues");
    }
    crypto.getRandomValues(out);
    return out;
}
function encodeBase64(bytes) {
    const btoa = hostGlobal().btoa;
    if (!btoa) {
        throw new PrnsValidationError("missing-host-api", "BrowserLocalStorageIdentityStore requires globalThis.btoa");
    }
    let binary = "";
    for (const byte of bytes) {
        binary += String.fromCharCode(byte);
    }
    return btoa(binary);
}
function decodeBase64(encoded) {
    const atob = hostGlobal().atob;
    if (!atob) {
        throw new PrnsValidationError("missing-host-api", "BrowserLocalStorageIdentityStore requires globalThis.atob");
    }
    const binary = atob(encoded);
    const out = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
        out[i] = binary.charCodeAt(i);
    }
    return out;
}
function requireWebUsb() {
    try {
        const usb = hostGlobal().navigator?.usb;
        return usb
            ? Tag("Available", usb)
            : Tag("HostApiUnavailable", { api: "WebUSB" });
    }
    catch {
        return Tag("HostApiUnavailable", { api: "WebUSB" });
    }
}
function requireWebBluetooth() {
    try {
        const bluetooth = hostGlobal().navigator?.bluetooth;
        return bluetooth
            ? Tag("Available", bluetooth)
            : Tag("HostApiUnavailable", { api: "WebBluetooth" });
    }
    catch {
        return Tag("HostApiUnavailable", { api: "WebBluetooth" });
    }
}
function requireBrowserWebSocket() {
    try {
        const WebSocketCtor = hostGlobal().WebSocket;
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
                timeoutMs: WEBSOCKET_CONNECT_TIMEOUT_MS,
            }));
        };
        try {
            timeout = globalThis.setTimeout(handleTimeout, WEBSOCKET_CONNECT_TIMEOUT_MS);
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
function closeBrowserWebSocket(socket) {
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
function firstUsbConfiguration(device) {
    const configuration = device.configurations[0];
    if (!configuration) {
        return Tag("UnsupportedDevice", {
            interface: "usb-auto",
            capability: "USB configuration",
        });
    }
    return Tag("Configured", configuration);
}
function findWebUsbEndpointPair(configuration) {
    const vendorPairs = [];
    const bulkPairs = [];
    let fallbackPair;
    for (const iface of configuration.interfaces) {
        for (const alternate of iface.alternates) {
            const inEndpoint = alternate.endpoints.find((endpoint) => endpoint.direction === "in" && endpoint.type === "bulk");
            const outEndpoint = alternate.endpoints.find((endpoint) => endpoint.direction === "out" && endpoint.type === "bulk");
            if (inEndpoint && outEndpoint) {
                const pair = {
                    interfaceNumber: iface.interfaceNumber,
                    alternate,
                    inEndpoint,
                    outEndpoint,
                };
                if (alternate.interfaceClass === 0xff) {
                    vendorPairs.push(pair);
                }
                else {
                    bulkPairs.push(pair);
                }
                continue;
            }
            const fallbackIn = alternate.endpoints.find((endpoint) => endpoint.direction === "in");
            const fallbackOut = alternate.endpoints.find((endpoint) => endpoint.direction === "out");
            if (!fallbackPair && fallbackIn && fallbackOut) {
                fallbackPair = {
                    interfaceNumber: iface.interfaceNumber,
                    alternate,
                    inEndpoint: fallbackIn,
                    outEndpoint: fallbackOut,
                };
            }
        }
    }
    return vendorPairs[0] ?? bulkPairs[0] ?? fallbackPair;
}
async function usbStage(stage, actionName, action) {
    try {
        return Tag("Completed", await action());
    }
    catch (error) {
        const name = domExceptionName(error);
        if (name === "SecurityError" || name === "NotAllowedError") {
            return Tag("PermissionDenied", {
                interface: "usb-auto",
                stage,
                detail: describeUsbError(error, actionName),
            });
        }
        if (name === "NotFoundError" && stage === "DeviceSelection") {
            return Tag("Cancelled", { interface: "usb-auto", stage });
        }
        return Tag("ConnectionFailed", {
            interface: "usb-auto",
            stage,
            detail: `USB ${actionName} failed: ${describeUsbError(error, actionName)}`,
        });
    }
}
async function bluetoothStage(stage, action) {
    try {
        return Tag("Completed", await action());
    }
    catch (error) {
        const name = domExceptionName(error);
        if (name === "SecurityError" || name === "NotAllowedError") {
            return Tag("PermissionDenied", {
                interface: "bluetooth",
                stage,
                detail: describeHostError(error),
            });
        }
        if (name === "NotFoundError" && stage === "DeviceSelection") {
            return Tag("Cancelled", { interface: "bluetooth", stage });
        }
        return Tag("ConnectionFailed", {
            interface: "bluetooth",
            stage,
            detail: describeHostError(error),
        });
    }
}
function describeUsbError(error, stage) {
    const base = describeHostError(error);
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
        return `${base}. ${LINUX_WEBUSB_SETUP_HINT}`;
    }
    if (name === "NotFoundError" && stage.includes("request")) {
        return `${base}. No USB device was selected.`;
    }
    return base;
}
async function closeUsbDevice(device) {
    try {
        await device.close();
        return undefined;
    }
    catch (error) {
        return Tag("TransportCloseFailed", {
            detail: `close USB device: ${describeHostError(error)}`,
        });
    }
}
function disconnectBluetoothServer(server) {
    try {
        server.disconnect();
        return undefined;
    }
    catch (error) {
        return Tag("TransportCloseFailed", {
            detail: `disconnect Bluetooth server: ${describeHostError(error)}`,
        });
    }
}
function domExceptionName(error) {
    return typeof DOMException !== "undefined" && error instanceof DOMException
        ? error.name
        : undefined;
}
function connectFailure(interfaceName, stage, error) {
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
        return Tag("PermissionDenied", {
            interface: interfaceName,
            stage,
            detail: describeHostError(error),
        });
    }
    if (name === "NotFoundError" && stage === "DeviceSelection") {
        return Tag("Cancelled", { interface: interfaceName, stage });
    }
    return Tag("ConnectionFailed", {
        interface: interfaceName,
        stage,
        detail: describeHostError(error),
    });
}
function describeHostError(error) {
    if (typeof DOMException !== "undefined" && error instanceof DOMException) {
        return `${error.name}: ${error.message}`;
    }
    if (error instanceof Error) {
        return `${error.name}: ${error.message}`;
    }
    return String(error);
}
async function optionalBluetoothCharacteristic(service, uuid) {
    try {
        return await service.getCharacteristic(uuid);
    }
    catch {
        return undefined;
    }
}
function characteristicBytes(event) {
    const value = event.target?.value;
    if (!value) {
        return Tag("ProtocolViolation", {
            protocol: "Bluetooth",
            detail: "Bluetooth characteristic event did not include a value",
        });
    }
    return Tag("Decoded", new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
}
async function writeBluetoothValue(characteristic, bytes) {
    const value = arrayBufferForBluetooth(bytes);
    try {
        if (characteristic.writeValueWithoutResponse) {
            await characteristic.writeValueWithoutResponse(value);
        }
        else if (characteristic.writeValueWithResponse) {
            await characteristic.writeValueWithResponse(value);
        }
        else if (characteristic.writeValue) {
            await characteristic.writeValue(value);
        }
        else {
            return Tag("TransferFailed", {
                direction: "Outbound",
                detail: "Bluetooth characteristic does not support writes",
            });
        }
        return Tag("Written");
    }
    catch (error) {
        return Tag("TransferFailed", {
            direction: "Outbound",
            detail: describeHostError(error),
        });
    }
}
function arrayBufferForBluetooth(bytes) {
    const out = new ArrayBuffer(bytes.length);
    new Uint8Array(out).set(bytes);
    return out;
}
function arrayBufferForUsb(bytes) {
    const out = new ArrayBuffer(bytes.length);
    new Uint8Array(out).set(bytes);
    return out;
}
function browserUsbAutoChannelTag(device) {
    const vendor = formatOptionalHex(device.vendorId);
    const product = formatOptionalHex(device.productId);
    const serial = device.serialNumber ?? "unknown";
    const nonce = nextBrowserUsbAutoTag;
    nextBrowserUsbAutoTag = (nextBrowserUsbAutoTag + 1) >>> 0;
    return channelTag(new TextEncoder().encode(`webusb:auto-usb:${vendor}:${product}:${serial}:${nonce}`));
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
function runtimeRejected(operation, error) {
    return Tag("RuntimeRejected", {
        operation,
        detail: describeHostError(error),
    });
}
function commandFailed(failure) {
    return Tag("Failed", failure);
}
function browserCommandFailure(operation, error) {
    const detail = describeHostError(error);
    if (detail.includes("payload exceeds")) {
        return Tag("PayloadTooLarge");
    }
    return Tag("WriteFailed", { detail: `${operation}: ${detail}` });
}
function runtimeResponseTimeout(timeout) {
    return match(timeout, {
        LinkDefault: () => ({}),
        Exact: ({ millis }) => ({
            timeoutMillis: nonNegativeInteger(millis, "timeoutMillis"),
        }),
    });
}
function runtimeResourceStrategy(strategy) {
    return match(strategy, {
        Refuse: () => ({ strategy: "refuse" }),
        Accept: ({ maximumUncompressedBytes, acceptCompressed, }) => ({
            strategy: "accept",
            maximumUncompressedBytes: nonNegativeInteger(maximumUncompressedBytes, "maximumUncompressedBytes"),
            acceptCompressed,
        }),
    });
}
function concatenateBytes(parts) {
    const length = parts.reduce((total, part) => total + part.length, 0);
    const joined = new Uint8Array(length);
    let offset = 0;
    for (const part of parts) {
        joined.set(part, offset);
        offset += part.length;
    }
    return joined;
}
function fillEntropy(source, length) {
    let outcome;
    try {
        outcome = source(length);
    }
    catch (error) {
        return Tag("EntropySourceFailed", { detail: describeHostError(error) });
    }
    if (outcome.tag !== "Filled") {
        return outcome;
    }
    if (outcome.data.length < length) {
        return Tag("InsufficientEntropy", {
            minimum: length,
            actual: outcome.data.length,
        });
    }
    return outcome;
}
function webCryptoIdentity(length) {
    try {
        if (!hostGlobal().crypto) {
            return Tag("HostApiUnavailable", { api: "Crypto" });
        }
        return Tag("Generated", identitySecretKey(webCryptoBytes(length), length));
    }
    catch (error) {
        return Tag("EntropySourceFailed", { detail: describeHostError(error) });
    }
}
async function loadOrCreateBleIdentity(store) {
    let loaded;
    try {
        loaded = await store.load(BLE_IDENTITY_LENGTH);
    }
    catch (error) {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `load Bluetooth LE identity: ${describeHostError(error)}`,
        });
    }
    if (loaded.tag === "Loaded") {
        const validated = bleIdentity(loaded.data);
        return validated.tag === "ValidBleIdentity"
            ? Tag("Available", validated.data)
            : Tag("StableIdentityUnavailable", {
                interface: "bluetooth",
                detail: `stored Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
            });
    }
    if (loaded.tag !== "Missing") {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: describeStableIdentityStoreFailure(loaded),
        });
    }
    let generatedBytes;
    try {
        generatedBytes = webCryptoBytes(BLE_IDENTITY_LENGTH);
    }
    catch (error) {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `generate Bluetooth LE identity: ${describeHostError(error)}`,
        });
    }
    const validated = bleIdentity(generatedBytes);
    if (validated.tag !== "ValidBleIdentity") {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `generated Bluetooth LE identity has ${validated.data.actualLength} bytes; expected ${BLE_IDENTITY_LENGTH}`,
        });
    }
    const generated = validated.data;
    let saved;
    try {
        saved = await store.save(generated);
    }
    catch (error) {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: `save Bluetooth LE identity: ${describeHostError(error)}`,
        });
    }
    if (saved.tag !== "Saved") {
        return Tag("StableIdentityUnavailable", {
            interface: "bluetooth",
            detail: describeStableIdentityStoreFailure(saved),
        });
    }
    return Tag("Available", generated);
}
function describeStableIdentityStoreFailure(failure) {
    return match_into().from(failure, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        StableIdentityStoreFailed: ({ operation, detail }) => `${operation} stable identity: ${detail}`,
        StoredStableIdentityInvalid: ({ detail }) => detail,
    });
}
function unexpectedSessionFailure(error) {
    return Tag("UnexpectedSessionFailure", { detail: describeHostError(error) });
}
function closeFailed(causes) {
    return Tag("CloseFailed", { causes });
}
function hasCleanupFailures(causes) {
    return causes.length > 0;
}
function closedSessionOutcome(status) {
    return status.tag === "Failed" && status.data.tag === "CloseFailed"
        ? status.data
        : Tag("Closed");
}
function sessionFailureToConnectFailure(interfaceName, stage, failure) {
    if (failure.tag === "RuntimeRejected") {
        return failure;
    }
    return Tag("ConnectionFailed", {
        interface: interfaceName,
        stage,
        detail: describeInterfaceSessionFailure(failure),
    });
}
function describeBluetoothConnectFailure(failure) {
    return match(failure, {
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        PermissionDenied: ({ detail }) => detail,
        Cancelled: ({ stage }) => `Bluetooth ${stage} was cancelled`,
        UnsupportedDevice: ({ capability }) => `Bluetooth device does not provide ${capability}`,
        TimedOut: ({ stage, timeoutMs }) => `Bluetooth ${stage} timed out after ${timeoutMs}ms`,
        ConnectionFailed: ({ detail }) => detail,
        AlreadyActive: ({ target }) => `${target} is already active`,
        StableIdentityUnavailable: ({ detail }) => detail,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
function describeInterfaceSessionFailure(failure) {
    return match_into().from(failure, {
        Disconnected: ({ detail }) => detail,
        UnexpectedSessionFailure: ({ detail }) => detail,
        EntropySourceFailed: ({ detail }) => detail,
        TransferFailed: ({ direction, detail }) => `${direction} transfer: ${detail}`,
        ProtocolViolation: ({ protocol, detail }) => `${protocol}: ${detail}`,
        UnsupportedFrame: ({ format }) => `unsupported ${format.toLowerCase()} frame`,
        FrameTooLarge: ({ length, maximum }) => `frame is ${length} bytes; maximum is ${maximum}`,
        OutboundQueueFull: ({ capacity }) => `outbound queue reached ${capacity} frames`,
        CloseFailed: ({ causes }) => causes.map((cause) => cause.data.detail).join("; "),
        HostApiUnavailable: ({ api }) => `${api} is unavailable`,
        InsufficientEntropy: ({ actual, minimum }) => `entropy source returned ${actual} bytes; minimum is ${minimum}`,
        RuntimeRejected: ({ operation, detail }) => `${operation}: ${detail}`,
    });
}
function normalizedWebSocketProtocols(protocols) {
    if (protocols === undefined || typeof protocols === "string") {
        return protocols;
    }
    return protocols.length === 0 ? undefined : [...protocols];
}
function browserWebSocketChannelTag(url, protocols) {
    const protocolList = protocols === undefined
        ? []
        : typeof protocols === "string"
            ? [protocols]
            : protocols;
    return channelTag(new TextEncoder().encode(JSON.stringify(["websocket-client", url, protocolList])));
}
function byteKey(bytes) {
    let key = "";
    for (const byte of bytes) {
        key += byte.toString(16).padStart(2, "0");
    }
    return key;
}
function formatOptionalHex(value) {
    return value === undefined ? "unknown" : value.toString(16).padStart(4, "0");
}
async function loadBundledWasm() {
    const moduleUrl = bundledWasmModuleUrl();
    try {
        const imported = await import(moduleUrl.href);
        const module = record(imported, "bundled WebAssembly module");
        const initialize = module.default;
        if (typeof initialize !== "function") {
            return Tag("WasmLoadFailed", {
                detail: "bundled WebAssembly module has no initializer",
            });
        }
        await initialize();
        return Tag("Loaded", imported);
    }
    catch (error) {
        return Tag("WasmLoadFailed", { detail: describeHostError(error) });
    }
}
function bundledWasmModuleUrl() {
    return new URL("../../wasm/prns_wasm.js", import.meta.url);
}
function browserLimits(limits) {
    return {
        pendingCommands: positiveInteger(limits.pendingCommands, "pending command limit"),
        applicationEvents: positiveInteger(limits.applicationEvents, "application event limit"),
        retainedEventBytes: positiveInteger(limits.retainedEventBytes, "retained event byte limit"),
        diagnostics: positiveInteger(limits.diagnostics, "diagnostic limit"),
    };
}
function retainedBrowserEventBytes(event) {
    return match_into().from(event, {
        SingleDelivery: ({ plaintext }) => plaintext.length,
        Request: ({ data }) => data.length,
        Response: ({ data }) => data.length,
        ResponseSegment: ({ data }) => data.length,
        ResourceAvailable: ({ resource, metadata }) => resource.totalBytes + (metadata?.length ?? 0),
        ResourceSegment: ({ data, metadata }) => data.length + (metadata?.length ?? 0),
        ResourceNeedsDecompression: ({ stream }) => stream.length,
        ChannelMessage: ({ data }) => data.length,
    });
}
function rawEventType(value) {
    if (!RAW_EVENT_TYPES.has(value)) {
        throw new PrnsValidationError("invalid-component", `runtime emitted event outside host contract: ${value}`);
    }
    return value;
}
const RAW_LINK_CLOSED_REASONS = new Set([
    "timeout",
    "peerClosed",
    "malformedRtt",
]);
function linkClosedReason(value) {
    if (!RAW_LINK_CLOSED_REASONS.has(value)) {
        throw new PrnsValidationError("invalid-component", `unknown link close reason ${value}`);
    }
    return match(value, {
        timeout: () => "Timeout",
        peerClosed: () => "PeerClosed",
        malformedRtt: () => "MalformedRtt",
    });
}
function delay(ms) {
    return new Promise((resolve) => {
        setTimeout(resolve, ms);
    });
}
function hostGlobal() {
    return globalThis;
}
function equalBytes(left, right) {
    if (left.length !== right.length) {
        return false;
    }
    for (let i = 0; i < left.length; i += 1) {
        if (left[i] !== right[i]) {
            return false;
        }
    }
    return true;
}

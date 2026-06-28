export const INTERFACE_ID_LENGTH = 8;
export const DESTINATION_HASH_LENGTH = 16;
export const MIN_ENTROPY_BYTES = 64;
export class PrnsValidationError extends Error {
    code;
    constructor(code, message) {
        super(message);
        this.name = "PrnsValidationError";
        this.code = code;
    }
}
const USB_AUTO_WEB_SERIAL_BAUD_RATE = 115_200;
const USB_AUTO_PROBE_INTERVAL_MS = 500;
const USB_AUTO_OUTBOUND_POLL_MS = 25;
const WEBUSB_MIN_TRANSFER_BYTES = 512;
const BLUETOOTH_HANDSHAKE_TIMEOUT_MS = 10_000;
const BLUETOOTH_OUTBOUND_POLL_MS = 25;
let nextBrowserUsbAutoTag = 0;
export class BrowserLocalStorageIdentityStore {
    #key;
    constructor(key = "prns.identity.v1") {
        this.#key = key;
    }
    async load(expectedLength) {
        const storage = requireLocalStorage();
        const encoded = storage.getItem(this.#key);
        return encoded ? identitySecretKey(decodeBase64(encoded), expectedLength) : null;
    }
    async save(secretKey) {
        requireLocalStorage().setItem(this.#key, encodeBase64(secretKey));
    }
}
export class PrnsInterfaces {
    usbAuto;
    rnode;
    bluetooth;
    constructor(host) {
        this.usbAuto = new UsbAutoInterface(host);
        this.rnode = new RNodeInterface(host);
        this.bluetooth = new BluetoothInterface(host);
    }
}
export class UsbAutoInterface {
    name = "usb-auto";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect(options = {}) {
        this.#host.assertReady();
        const usb = requireWebUsb();
        const device = await usbStage("request browser USB device", () => usb.requestDevice({
            filters: options.filters ?? this.#host.defaultUsbAutoFilters(),
        }));
        const transport = await WebUsbAutoTransport.open(device);
        let session;
        try {
            const interfaceId = this.#host.registerInterface({
                kind: "auto-usb-host",
                channelTag: browserUsbAutoChannelTag(device),
                bitrateBps: this.#host.usbAutoHostBitrateBps(),
                hardwareMtu: this.#host.usbAutoHostHardwareMtu(),
            });
            session = new BrowserUsbAutoSession(this.#host, transport, interfaceId);
            session.start();
            return session;
        }
        catch (error) {
            await session?.close();
            if (!session) {
                await transport.close();
            }
            throw error;
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
    #writeQueue = Promise.resolve();
    #closed = false;
    #confirmed = false;
    #state = "port-open";
    #failure;
    constructor(host, transport, interfaceId) {
        this.#host = host;
        this.#transport = transport;
        this.interfaceId = interfaceId;
        this.#decoder = host.createUsbAutoDecoder();
        this.#nodeTag = host.usbAutoNodeTagFor(interfaceId);
    }
    get state() {
        return this.#state;
    }
    get failure() {
        return this.#failure;
    }
    get peerConfirmed() {
        return this.#confirmed;
    }
    start() {
        void this.#readLoop();
        void this.#probeLoop();
        void this.#outboundLoop();
    }
    async close() {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        if (this.#state !== "failed") {
            this.#state = "closed";
        }
        await this.#writeQueue.catch(ignoreError);
        await this.#transport.close();
    }
    async #readLoop() {
        try {
            while (!this.#closed) {
                const chunk = await this.#transport.read();
                if (!chunk) {
                    break;
                }
                if (chunk.length === 0) {
                    continue;
                }
                for (const raw of this.#decoder.feed(chunk)) {
                    await this.#handleInbound(parseUsbAutoMessage(raw));
                }
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(error);
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
                this.#state = "handshaking";
                await this.#writeFrame(this.#host.usbAutoHostHelloFrame());
                await delay(USB_AUTO_PROBE_INTERVAL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(error);
            }
        }
    }
    async #outboundLoop() {
        try {
            while (!this.#closed) {
                if (this.#confirmed) {
                    const frames = this.#host.takeOutboundFor(this.interfaceId, "auto-usb-host");
                    for (const frame of frames) {
                        await this.#writeFrame(this.#host.usbAutoDataFrame(frame.bytes));
                    }
                }
                await delay(USB_AUTO_OUTBOUND_POLL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(error);
            }
        }
    }
    async #handleInbound(message) {
        switch (message.type) {
            case "hello":
                await this.#writeFrame(this.#host.usbAutoHostHelloAckFrame(this.#nodeTag));
                this.#confirmPeer();
                break;
            case "helloAck":
                this.#confirmPeer();
                break;
            case "data":
                if (this.#confirmed && message.bytes.length > 0) {
                    this.#host.ingest(this.interfaceId, packetFrame(message.bytes));
                }
                break;
        }
    }
    #confirmPeer() {
        this.#confirmed = true;
        this.#state = "peer-confirmed";
    }
    async #fail(error) {
        this.#failure = interfaceFailure(error);
        this.#state = "failed";
        this.#closed = true;
        await this.#writeQueue.catch(ignoreError);
        await this.#transport.close();
    }
    async #writeFrame(frame) {
        if (this.#closed) {
            return;
        }
        const write = this.#writeQueue.then(async () => {
            if (!this.#closed) {
                await this.#transport.write(frame);
            }
        });
        this.#writeQueue = write.catch(ignoreError);
        await write;
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
        await usbStage("open selected USB device", () => device.open());
        const configuration = device.configuration ?? firstUsbConfiguration(device);
        if (!device.configuration) {
            await usbStage(`select USB configuration ${configuration.configurationValue}`, () => device.selectConfiguration(configuration.configurationValue));
        }
        const selectedConfiguration = device.configuration ?? configuration;
        const endpoints = findWebUsbEndpointPair(selectedConfiguration);
        if (!endpoints) {
            await device.close().catch(ignoreError);
            throw new PrnsValidationError("invalid-component", "Selected USB device has no usable IN/OUT endpoint pair");
        }
        await usbStage(`claim USB interface ${endpoints.interfaceNumber}`, () => device.claimInterface(endpoints.interfaceNumber));
        if (endpoints.alternate.alternateSetting !== 0 &&
            device.selectAlternateInterface) {
            await usbStage(`select USB alternate ${endpoints.alternate.alternateSetting} on interface ${endpoints.interfaceNumber}`, () => device.selectAlternateInterface(endpoints.interfaceNumber, endpoints.alternate.alternateSetting));
        }
        return new WebUsbAutoTransport(device, endpoints.interfaceNumber, endpoints.inEndpoint, endpoints.outEndpoint);
    }
    async read() {
        if (this.#closed) {
            return undefined;
        }
        const length = Math.max(this.#inEndpoint.packetSize, WEBUSB_MIN_TRANSFER_BYTES);
        const result = await this.#device.transferIn(this.#inEndpoint.endpointNumber, length);
        if (result.status !== "ok") {
            throw new PrnsValidationError("transfer-failed", `USB IN transfer failed with status ${result.status}`);
        }
        const data = result.data;
        if (!data) {
            return new Uint8Array();
        }
        return new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    }
    async write(bytes) {
        if (this.#closed || bytes.length === 0) {
            return;
        }
        const result = await this.#device.transferOut(this.#outEndpoint.endpointNumber, arrayBufferForUsb(bytes));
        if (result.status !== "ok" || result.bytesWritten !== bytes.length) {
            throw new PrnsValidationError("transfer-failed", `USB OUT transfer wrote ${result.bytesWritten}/${bytes.length} bytes with status ${result.status}`);
        }
    }
    async close() {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        await this.#device.releaseInterface(this.#interfaceNumber).catch(ignoreError);
        await this.#device.close().catch(ignoreError);
    }
}
export class RNodeInterface {
    name = "rnode";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect() {
        this.#host.assertReady();
        throw new PrnsValidationError("missing-host-api", "RNode browser connection is not wired yet");
    }
}
export class BluetoothInterface {
    name = "bluetooth";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect() {
        this.#host.assertReady();
        const bluetooth = requireWebBluetooth();
        const serviceUuid = this.#host.bluetoothServiceUuid();
        const device = await bluetooth.requestDevice({
            filters: [{ services: [serviceUuid] }],
            optionalServices: [serviceUuid],
        });
        const server = await device.gatt?.connect();
        if (!server) {
            throw new PrnsValidationError("missing-host-api", "Web Bluetooth device did not expose a GATT server");
        }
        const service = await server.getPrimaryService(serviceUuid);
        const control = await service.getCharacteristic(this.#host.bluetoothControlUuid());
        const data = await optionalBluetoothCharacteristic(service, this.#host.bluetoothDataUuid());
        const session = new BrowserBluetoothSession(this.#host, server, control, data ?? control);
        await session.start();
        return session;
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
    #writeQueue = Promise.resolve();
    #closed = false;
    #confirmed = false;
    #state = "opening";
    #failure;
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
    get state() {
        return this.#state;
    }
    get failure() {
        return this.#failure;
    }
    get peerConfirmed() {
        return this.#confirmed;
    }
    async start() {
        this.#state = "handshaking";
        await this.#control.startNotifications();
        this.#control.addEventListener("characteristicvaluechanged", (event) => {
            this.#handleControlEvent(event);
        });
        if (this.#data !== this.#control) {
            await this.#data.startNotifications();
            this.#data.addEventListener("characteristicvaluechanged", (event) => {
                this.#handleDataEvent(event);
            });
        }
        await this.#writeControl(this.#host.bluetoothDialerHello());
        await this.#waitForPeer();
        void this.#outboundLoop();
    }
    async close() {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        if (this.#state !== "failed") {
            this.#state = "closed";
        }
        await this.#writeQueue.catch(ignoreError);
        this.#server.disconnect();
    }
    async #waitForPeer() {
        const started = Date.now();
        while (!this.#confirmed && !this.#closed) {
            if (Date.now() - started > BLUETOOTH_HANDSHAKE_TIMEOUT_MS) {
                await this.close();
                throw new PrnsValidationError("invalid-component", "Bluetooth handshake timed out before Welcome");
            }
            await delay(25);
        }
    }
    #handleControlEvent(event) {
        const bytes = characteristicBytes(event);
        const control = parseBluetoothControl(this.#host.bluetoothDecodeControl(bytes));
        if (control.type === "welcome") {
            this.#interfaceId = this.#host.registerInterface({
                kind: "bluetooth-peer",
                channelTag: channelTag(control.identity),
                bitrateBps: this.#host.bluetoothBitrateBps(),
                hardwareMtu: this.#host.bluetoothHardwareMtu(),
            });
            this.#confirmed = true;
            this.#state = "peer-confirmed";
            return;
        }
        if (control.type === "close") {
            void this.close();
            return;
        }
        if (this.#data === this.#control) {
            this.#handleDataBytes(bytes);
        }
    }
    #handleDataEvent(event) {
        this.#handleDataBytes(characteristicBytes(event));
    }
    #handleDataBytes(bytes) {
        if (!this.#confirmed || !this.#interfaceId) {
            return;
        }
        const frame = this.#reassembler.absorb(bytes);
        if (frame && frame.length > 0) {
            this.#host.ingest(this.#interfaceId, packetFrame(frame));
        }
    }
    async #outboundLoop() {
        try {
            while (!this.#closed) {
                const interfaceId = this.#interfaceId;
                if (this.#confirmed && interfaceId) {
                    const frames = this.#host.takeOutboundFor(interfaceId, "bluetooth-auto");
                    for (const frame of frames) {
                        for (const fragment of this.#host.bluetoothDataFragments(frame.bytes)) {
                            await this.#writeData(fragment);
                        }
                    }
                }
                await delay(BLUETOOTH_OUTBOUND_POLL_MS);
            }
        }
        catch (error) {
            if (!this.#closed) {
                await this.#fail(error);
            }
        }
    }
    async #fail(error) {
        this.#failure = interfaceFailure(error);
        this.#state = "failed";
        this.#closed = true;
        await this.#writeQueue.catch(ignoreError);
        this.#server.disconnect();
    }
    async #writeControl(bytes) {
        await this.#write(this.#control, bytes);
    }
    async #writeData(bytes) {
        await this.#write(this.#data, bytes);
    }
    async #write(characteristic, bytes) {
        const write = this.#writeQueue.then(async () => {
            if (!this.#closed) {
                await writeBluetoothValue(characteristic, bytes);
            }
        });
        this.#writeQueue = write.catch(ignoreError);
        await write;
    }
}
export class Prns {
    interfaces;
    #runtime;
    #entropy;
    #now;
    constructor(wasm, runtime, entropy, now) {
        this.#runtime = runtime;
        this.#entropy = entropy;
        this.#now = now;
        this.interfaces = new PrnsInterfaces(new RuntimeHost(wasm, runtime, entropy, now));
    }
    static async create(options) {
        const identityLength = options.wasm.identitySecretKeyLength();
        const store = options.identityStore;
        let identity = store ? await store.load(identityLength) : null;
        if (!identity) {
            identity = identitySecretKey(webCryptoBytes(identityLength), identityLength);
            await store?.save(identity);
        }
        return new Prns(options.wasm, new options.wasm.PrnsRuntime(identity), options.entropy ?? webCryptoBytes, options.now ?? nowMillis);
    }
    registerSingleDestination(options) {
        return destinationHash(this.#runtime.registerSingleDestination(options));
    }
    announce(destination) {
        return commandId(this.#runtime.announce({
            destination,
            nowMs: this.#now(),
            entropy: this.#entropyBytes(),
        }));
    }
    drainEvents() {
        return this.#runtime.drainEvents().map(parseEvent);
    }
    snapshot() {
        return parseSnapshot(this.#runtime.snapshot());
    }
    #entropyBytes() {
        return entropyBytes(this.#entropy(MIN_ENTROPY_BYTES));
    }
}
class RuntimeHost {
    #wasm;
    #runtime;
    #entropy;
    #now;
    #pendingOutbound = [];
    constructor(wasm, runtime, entropy, now) {
        this.#wasm = wasm;
        this.#runtime = runtime;
        this.#entropy = entropy;
        this.#now = now;
    }
    assertReady() {
        this.#runtime.snapshot();
    }
    registerInterface(options) {
        return interfaceId(this.#runtime.registerInterface(options));
    }
    ingest(interfaceId, bytes) {
        this.#runtime.ingest({
            interfaceId,
            bytes,
            nowMs: this.#now(),
            entropy: this.entropy(),
        });
    }
    drainOutbound() {
        return this.#runtime.drainOutbound().map(parseOutboundFrame);
    }
    takeOutboundFor(interfaceId, supervisorKind) {
        this.#pendingOutbound.push(...this.drainOutbound());
        const picked = [];
        const pending = [];
        for (const frame of this.#pendingOutbound) {
            if (outboundTargets(frame.target, interfaceId, supervisorKind)) {
                picked.push(frame);
            }
            else {
                pending.push(frame);
            }
        }
        this.#pendingOutbound = pending;
        return picked;
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
        return entropyBytes(this.#entropy(MIN_ENTROPY_BYTES));
    }
}
export function identitySecretKey(bytes, expectedLength) {
    return exactBytes(bytes, expectedLength, "IdentitySecretKey");
}
export function interfaceId(bytes) {
    return exactBytes(bytes, INTERFACE_ID_LENGTH, "InterfaceId");
}
export function destinationHash(bytes) {
    return exactBytes(bytes, DESTINATION_HASH_LENGTH, "DestinationHash");
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
    return entropyBytes(webCryptoBytes(length));
}
function outboundTargets(target, interfaceId, supervisorKind) {
    if (target.type === "interface") {
        return equalBytes(target.interfaceId, interfaceId);
    }
    if (target.supervisorKind !== supervisorKind) {
        return false;
    }
    switch (target.fan.type) {
        case "all":
            return true;
        case "only":
            return equalBytes(target.fan.interfaceId, interfaceId);
        case "allExcept":
            return !equalBytes(target.fan.interfaceId, interfaceId);
    }
}
function parseUsbAutoMessage(raw) {
    const object = record(raw, "UsbAutoInboundMessage");
    const type = stringField(object, "type");
    switch (type) {
        case "hello":
            return { type };
        case "helloAck":
            return { type, tag: bytesField(object, "tag") };
        case "data":
            return { type, bytes: bytesField(object, "bytes") };
        default:
            throw new PrnsValidationError("invalid-component", `unknown USB-auto message ${type}`);
    }
}
function parseBluetoothControl(raw) {
    const object = record(raw, "BluetoothControl");
    const type = stringField(object, "type");
    switch (type) {
        case "hello":
        case "welcome":
            return { type, identity: bytesField(object, "identity") };
        case "close":
            return { type, reason: stringField(object, "reason") };
        default:
            throw new PrnsValidationError("invalid-component", `unknown Bluetooth control ${type}`);
    }
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
        return {
            type,
            interfaceId: interfaceId(bytesField(object, "interfaceId")),
        };
    }
    if (type === "broadcast") {
        return {
            type,
            supervisorKind: parseRuntimeInterfaceKind(stringField(object, "supervisorKind")),
            fan: parseFanTarget(field(object, "fan")),
        };
    }
    throw new PrnsValidationError("unknown-outbound-target", `unknown outbound target ${type}`);
}
function parseFanTarget(raw) {
    const object = record(raw, "FanTarget");
    const type = stringField(object, "type");
    if (type === "all") {
        return { type };
    }
    if (type === "only" || type === "allExcept") {
        return {
            type,
            interfaceId: interfaceId(bytesField(object, "interfaceId")),
        };
    }
    throw new PrnsValidationError("unknown-outbound-target", `unknown fan target ${type}`);
}
function parseEvent(raw) {
    const object = record(raw, "PrnsEvent");
    const type = stringField(object, "type");
    switch (type) {
        case "announce":
            return {
                type,
                destination: destinationHash(bytesField(object, "destination")),
                hops: hopCount(numberField(object, "hops")),
                sourceInterface: interfaceId(bytesField(object, "sourceInterface")),
            };
        case "commandSettled":
            return {
                type,
                commandId: commandId(bigintField(object, "id")),
                debugSettlement: stringField(object, "settlement"),
            };
        case "routeExpired":
        case "routeEvicted":
        case "routeInterfaceGone":
            return {
                type,
                destination: destinationHash(bytesField(object, "destination")),
            };
        default:
            return { type: "unknown", raw };
    }
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
function requireWebSerial() {
    const serial = hostGlobal().navigator?.serial;
    if (!serial) {
        throw new PrnsValidationError("missing-host-api", "USB Auto requires the browser Web Serial API");
    }
    return serial;
}
function requireWebUsb() {
    const usb = hostGlobal().navigator?.usb;
    if (!usb) {
        throw new PrnsValidationError("missing-host-api", "USB Auto requires the browser WebUSB API");
    }
    return usb;
}
function requireWebBluetooth() {
    const bluetooth = hostGlobal().navigator?.bluetooth;
    if (!bluetooth) {
        throw new PrnsValidationError("missing-host-api", "Bluetooth requires the browser Web Bluetooth API");
    }
    return bluetooth;
}
function firstUsbConfiguration(device) {
    const configuration = device.configurations[0];
    if (!configuration) {
        throw new PrnsValidationError("invalid-component", "Selected USB device has no configurations");
    }
    return configuration;
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
async function usbStage(stage, action) {
    try {
        return await action();
    }
    catch (error) {
        const code = usbErrorCode(error);
        throw new PrnsValidationError(code, `USB ${stage} failed: ${describeUsbError(error, stage)}`);
    }
}
function usbErrorCode(error) {
    if (error instanceof DOMException) {
        if (error.name === "SecurityError") {
            return "permission-denied";
        }
        if (error.name === "NotFoundError") {
            return "operation-cancelled";
        }
        if (error.name === "NetworkError" ||
            error.name === "NotReadableError" ||
            error.name === "AbortError") {
            return "disconnected";
        }
    }
    return "invalid-component";
}
function describeUsbError(error, stage) {
    const base = describeHostError(error);
    if (error instanceof DOMException && error.name === "SecurityError") {
        return `${base}. On Linux, install the Prns WebUSB udev rule and replug the device.`;
    }
    if (error instanceof DOMException &&
        error.name === "NotFoundError" &&
        stage.includes("request")) {
        return `${base}. No USB device was selected.`;
    }
    return base;
}
function interfaceFailure(error) {
    if (error instanceof PrnsValidationError) {
        return { code: error.code, message: error.message };
    }
    if (error instanceof DOMException) {
        return {
            code: usbErrorCode(error),
            message: describeHostError(error),
        };
    }
    if (error instanceof Error) {
        return { code: "invalid-component", message: error.message };
    }
    return { code: "invalid-component", message: String(error) };
}
function describeHostError(error) {
    if (error instanceof DOMException) {
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
        throw new PrnsValidationError("invalid-component", "Bluetooth characteristic event did not include a value");
    }
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
}
async function writeBluetoothValue(characteristic, bytes) {
    const value = arrayBufferForBluetooth(bytes);
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
        throw new PrnsValidationError("missing-host-api", "Bluetooth characteristic does not support writes");
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
function formatOptionalHex(value) {
    return value === undefined ? "unknown" : value.toString(16).padStart(4, "0");
}
async function closeSerialPortQuietly(port) {
    await port.close().catch(ignoreError);
}
function delay(ms) {
    return new Promise((resolve) => {
        setTimeout(resolve, ms);
    });
}
function ignoreError(_error) { }
function requireLocalStorage() {
    const storage = hostGlobal().localStorage;
    if (!storage) {
        throw new PrnsValidationError("missing-host-api", "BrowserLocalStorageIdentityStore requires globalThis.localStorage");
    }
    return storage;
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

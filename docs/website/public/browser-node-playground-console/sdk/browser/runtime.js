import { Tag } from "../casework.js";
import { interfaceId } from "../contract.js";
import { byteKey } from "./bytes.js";
import { describeHostError } from "./host_errors.js";
import { outboundTargets, parseOutboundFrame, } from "./outbound.js";
import { MIN_ENTROPY_BYTES, PrnsValidationError, bitrateBps, channelTag, hardwareMtu, packetFrame, positiveInteger, } from "./values.js";
const INTERFACE_OUTBOUND_QUEUE_DEPTH = 64;
export class RuntimeHost {
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
        const { interfaceName, supervisorKind = registration.kind, contractKind = stableInterfaceKind(registration.kind), ...options } = registration;
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
        this.#activeInterfaces.set(key, {
            id,
            name: interfaceName,
            ...(contractKind === undefined ? {} : { contractKind }),
            registrationKey,
            supervisorKind,
            rxBytes: 0,
            txBytes: 0,
        });
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
    setContractKind(id, kind) {
        const active = this.#activeInterfaces.get(byteKey(id));
        if (active !== undefined) {
            active.contractKind = kind;
        }
    }
    interfaceInspection() {
        return new Map([...this.#activeInterfaces].map(([key, active]) => [
            key,
            {
                id: active.id,
                name: active.name,
                ...(active.contractKind === undefined
                    ? {}
                    : { kind: active.contractKind }),
                rxBytes: active.rxBytes,
                txBytes: active.txBytes,
            },
        ]));
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
            const active = this.#activeInterfaces.get(byteKey(interfaceId));
            if (active !== undefined) {
                active.rxBytes = saturatingAdd(active.rxBytes, bytes.length);
            }
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
    takeOutboundFor(interfaceId, maximumFrames = Number.MAX_SAFE_INTEGER) {
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
        const available = queued.concat(direct);
        const outbound = available.slice(0, maximumFrames);
        this.#outboundQueues.set(interfaceKey, available.slice(maximumFrames));
        const active = this.#activeInterfaces.get(interfaceKey);
        if (active !== undefined) {
            active.txBytes = outbound.reduce((total, frame) => saturatingAdd(total, frame.bytes.length), active.txBytes);
        }
        return Tag("Outbound", outbound);
    }
    createUsbAutoDecoder() {
        return new this.#wasm.UsbAutoDecoder();
    }
    createBluetoothReassembler() {
        return new this.#wasm.BluetoothReassembler();
    }
    createWebSocketFramingCodec(selection) {
        return new this.#wasm.WebSocketFramingCodec(wasmWebSocketFramingSelection(selection));
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
    webSocketRegister(options) {
        try {
            return this.registerInterface({
                interfaceName: "websocket",
                kind: "websocket-client",
                channelTag: channelTag(options.channelTag),
                bitrateBps: options.bitrateBps,
                hardwareMtu: options.hardwareMtu,
                ...runtimeInterfaceRouting(options.routing),
            });
        }
        catch (error) {
            return runtimeRejected("register-interface", error);
        }
    }
    webSocketIngest(id, bytes) {
        try {
            return this.ingest(id, packetFrame(bytes));
        }
        catch (error) {
            return runtimeRejected("ingest", error);
        }
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
export function runtimeRejected(operation, error) {
    return Tag("RuntimeRejected", {
        operation,
        detail: describeHostError(error),
    });
}
export function fillEntropy(source, length) {
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
export function saturatingAdd(left, right) {
    return Math.min(Number.MAX_SAFE_INTEGER, left + right);
}
function runtimeInterfaceRouting(routing) {
    if (routing === undefined)
        return {};
    if (routing.gravity !== undefined && !Number.isSafeInteger(routing.gravity)) {
        throw new PrnsValidationError("invalid-number", "gravity must be a safe integer");
    }
    return {
        ...(routing.mode === undefined ? {} : { mode: routing.mode }),
        ...(routing.gravity === undefined ? {} : { gravity: routing.gravity }),
        ...(routing.recursivePathRequests === undefined
            ? {}
            : { recursivePathRequests: routing.recursivePathRequests }),
        ...(routing.announcesFromInternal === undefined
            ? {}
            : { announcesFromInternal: routing.announcesFromInternal }),
        ...(routing.announcesToInternal === undefined
            ? {}
            : { announcesToInternal: routing.announcesToInternal }),
    };
}
function stableInterfaceKind(kind) {
    return {
        "auto-usb-host": "AutomaticUsb",
        "auto-usb-device": "AutomaticUsb",
        rnode: "RNode",
        "bluetooth-auto": "AutomaticBluetoothLe",
        "bluetooth-peer": "AutomaticBluetoothLe",
        "auto-wifi": "BrowserRendezvous",
        "websocket-client": "WebSocketClient",
        "websocket-server": "WebSocketServer",
        "websocket-server-peer": "WebSocketServer",
        serial: "Serial",
        kiss: "Kiss",
        pipe: "Pipe",
    }[kind];
}
function wasmWebSocketFramingSelection(selection) {
    switch (selection) {
        case "Auto":
            return "auto";
        case "RawPacket":
            return "raw";
        case "Hdlc":
            return "hdlc";
        case "Kiss":
            return "kiss";
    }
    const unreachable = selection;
    return unreachable;
}

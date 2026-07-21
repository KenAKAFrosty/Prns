import init, * as wasm from "./pkg/prns_wasm.js";
import { Prns, Tag } from "./sdk/index.js";
import { BROWSER_PLAYGROUND_LXMF_DELIVERY, LXMF_DELIVERY_DISPLAY_NAME, } from "./lxmf.js";
import { describeAutoWifiFailure, describeEntropyFailure, describeHostError, describeHostOperationFailure, describeRuntimeRejected, describeStartupFailure, describeUnknownOutcome, describeUsbCloseFailure, describeUsbConnectFailure, hostOperationFailed, } from "./outcomes.js";
import { hex, presentPacketContent } from "./presentation.js";
import { controlAvailability, sameAutoWifiStatus, } from "./state.js";
import { PlaygroundView, bindPlaygroundView, renderBindingFailure, } from "./view.js";
const POLL_INTERVAL_MS = 250;
const WASM_BINARY_PATH = "./pkg/prns_wasm_bg.wasm";
class BrowserPlayground {
    #view;
    #prns;
    #destination;
    #autoWifi = Tag("Waiting");
    #usb = Tag("Waiting");
    #snapshot;
    #pollTimer;
    #lastRuntimeFailure = "";
    #closed = false;
    constructor(view, prns, destination) {
        this.#view = view;
        this.#prns = prns;
        this.#destination = destination;
    }
    static async start(view) {
        if (BROWSER_PLAYGROUND_LXMF_DELIVERY.tag !== "Prepared") {
            return BROWSER_PLAYGROUND_LXMF_DELIVERY;
        }
        try {
            await init({
                module_or_path: new URL(WASM_BINARY_PATH, globalThis.location.href),
            });
        }
        catch (error) {
            return Tag("WasmLoadFailed", { detail: describeHostError(error) });
        }
        let created;
        try {
            created = await Prns.create({ wasm: wasmModule() });
        }
        catch (error) {
            return hostOperationFailed("Create runtime", error);
        }
        if (created.tag !== "Ready") {
            return created;
        }
        const registered = created.data.registerSingleDestination(BROWSER_PLAYGROUND_LXMF_DELIVERY.data.registration);
        if (registered.tag !== "Registered") {
            return registered;
        }
        const playground = new BrowserPlayground(view, created.data, registered.data);
        playground.#run();
        return Tag("Running", playground);
    }
    async close() {
        if (this.#closed) {
            return;
        }
        this.#closed = true;
        if (this.#pollTimer !== undefined) {
            globalThis.clearInterval(this.#pollTimer);
            this.#pollTimer = undefined;
        }
        const usb = usbSession(this.#usb);
        const autoWifi = this.#autoWifi.tag === "Running"
            ? this.#autoWifi.data.controller
            : undefined;
        this.#usb = Tag("Closed");
        this.#autoWifi = Tag("Closed");
        await Promise.allSettled([usb?.close(), autoWifi?.close()]);
    }
    #run() {
        this.#autoWifi = Tag("Ready");
        this.#usb = webUsbAvailable()
            ? Tag("Ready")
            : Tag("Unavailable", { api: "WebUSB" });
        this.#view.renderRuntimeReady(this.#destination);
        this.#view.renderAutoWifi(this.#autoWifi);
        this.#view.renderUsb(this.#usb);
        this.#view.record("Runtime", "Browser node runtime ready", `${LXMF_DELIVERY_DISPLAY_NAME} · lxmf.delivery ${hex(this.#destination)}`);
        this.#view.bindControls({
            startAutoWifi: () => this.#startAutoWifi(),
            closeAutoWifi: () => {
                void this.#closeAutoWifi();
            },
            connectUsb: () => {
                void this.#connectUsb();
            },
            closeUsb: () => {
                void this.#closeUsb();
            },
            announce: () => this.#announce(),
            clearActivity: () => this.#view.clearActivity(),
        });
        globalThis.addEventListener("pagehide", () => {
            void this.close();
        });
        this.#pollTimer = globalThis.setInterval(() => {
            this.#poll();
        }, POLL_INTERVAL_MS);
        this.#poll();
    }
    #startAutoWifi() {
        switch (this.#autoWifi.tag) {
            case "Ready":
            case "Closed":
                break;
            case "Waiting":
            case "Running":
                return;
            default:
                return unknownState(this.#autoWifi);
        }
        const controller = this.#prns.interfaces.autoWifi.start();
        this.#autoWifi = Tag("Running", {
            controller,
            status: controller.status,
        });
        this.#view.renderAutoWifi(this.#autoWifi);
        this.#recordAutoWifiStatus(controller.status);
        this.#view.record("Auto Wi-Fi", "Discovery started", "Probing localhost, prns.local, and their local gateway catalogs.");
        this.#syncControls();
    }
    async #closeAutoWifi() {
        if (this.#autoWifi.tag !== "Running") {
            return;
        }
        const controller = this.#autoWifi.data.controller;
        try {
            const outcome = await controller.close();
            switch (outcome.tag) {
                case "Closed":
                    this.#autoWifi = Tag("Closed");
                    this.#view.record("Auto Wi-Fi", "Transport closed", null);
                    break;
                case "RuntimeRejected":
                    this.#view.record("Failure", "Auto Wi-Fi close was rejected", describeRuntimeRejected(outcome));
                    break;
                default:
                    this.#view.record("Failure", "Auto Wi-Fi returned an unknown close outcome", describeUnknownOutcome("Auto Wi-Fi close", outcome));
            }
        }
        catch (error) {
            const outcome = hostOperationFailed("Close Auto Wi-Fi", error);
            this.#view.record("Failure", "Auto Wi-Fi close failed", describeHostOperationFailure(outcome));
        }
        this.#pollAutoWifi();
        this.#view.renderAutoWifi(this.#autoWifi);
        this.#syncControls();
    }
    async #connectUsb() {
        switch (this.#usb.tag) {
            case "Ready":
            case "ConnectFailed":
            case "Closed":
                break;
            case "Waiting":
            case "Unavailable":
            case "Connecting":
            case "Connected":
            case "Closing":
            case "CloseFailed":
                return;
            default:
                return unknownState(this.#usb);
        }
        this.#usb = Tag("Connecting");
        this.#view.renderUsb(this.#usb);
        this.#syncControls();
        this.#view.record("USB Auto", "Device selection opened", "Choose a Prns USB Auto device in the browser prompt.");
        let outcome;
        try {
            outcome = await this.#prns.interfaces.usbAuto.connect();
        }
        catch (error) {
            const failure = hostOperationFailed("Connect USB Auto", error);
            this.#usb = Tag("ConnectFailed", failure);
            this.#view.renderUsb(this.#usb);
            this.#view.record("Failure", "USB Auto did not connect", describeHostOperationFailure(failure));
            this.#syncControls();
            return;
        }
        switch (outcome.tag) {
            case "Connected":
                this.#usb = Tag("Connected", outcome.data);
                this.#view.record("USB Auto", "Session opened", `Interface ${hex(outcome.data.interfaceId)}`);
                break;
            case "HostApiUnavailable":
            case "PermissionDenied":
            case "Cancelled":
            case "AlreadyActive":
            case "UnsupportedDevice":
            case "ConnectionFailed":
            case "RuntimeRejected":
                this.#usb = Tag("ConnectFailed", outcome);
                this.#view.record("Failure", "USB Auto did not connect", describeUsbConnectFailure(outcome));
                break;
            default:
                this.#view.record("Failure", "USB Auto returned an unknown connection outcome", describeUnknownOutcome("USB Auto connection", outcome));
        }
        this.#view.renderUsb(this.#usb);
        this.#syncControls();
    }
    async #closeUsb() {
        const session = usbClosableSession(this.#usb);
        if (!session) {
            return;
        }
        this.#usb = Tag("Closing", session);
        this.#view.renderUsb(this.#usb);
        this.#syncControls();
        let outcome;
        try {
            outcome = await session.close();
        }
        catch (error) {
            const failure = hostOperationFailed("Close USB Auto", error);
            this.#usb = Tag("CloseFailed", { session, failure });
            this.#view.renderUsb(this.#usb);
            this.#view.record("Failure", "USB Auto close failed", describeHostOperationFailure(failure));
            this.#syncControls();
            return;
        }
        switch (outcome.tag) {
            case "Closed":
                this.#usb = Tag("Closed");
                this.#view.record("USB Auto", "Session closed", null);
                break;
            case "CloseFailed":
                this.#usb = Tag("CloseFailed", { session, failure: outcome });
                this.#view.record("Failure", "USB Auto close failed", describeUsbCloseFailure(outcome));
                break;
            default:
                this.#view.record("Failure", "USB Auto returned an unknown close outcome", describeUnknownOutcome("USB Auto close", outcome));
        }
        this.#view.renderUsb(this.#usb);
        this.#syncControls();
    }
    #announce() {
        if ((this.#snapshot?.interfaces.length ?? 0) === 0) {
            return;
        }
        const outcome = this.#prns.announce(this.#destination);
        switch (outcome.tag) {
            case "Queued":
                this.#view.record("Announce", "LXMF delivery announce queued", `Command ${outcome.data.toString()}`);
                return;
            case "HostApiUnavailable":
            case "EntropySourceFailed":
            case "InsufficientEntropy":
                this.#view.record("Failure", "LXMF delivery announce was not queued", describeEntropyFailure(outcome));
                return;
            case "RuntimeRejected":
                this.#view.record("Failure", "LXMF delivery announce was rejected", describeRuntimeRejected(outcome));
                return;
            default:
                this.#view.record("Failure", "Announce returned an unknown outcome", describeUnknownOutcome("announce", outcome));
        }
    }
    #poll() {
        if (this.#closed) {
            return;
        }
        this.#pollAutoWifi();
        this.#pollUsb();
        this.#pollRuntime();
        this.#syncControls();
    }
    #pollAutoWifi() {
        if (this.#autoWifi.tag !== "Running") {
            return;
        }
        const current = this.#autoWifi.data;
        const status = current.controller.status;
        if (sameAutoWifiStatus(current.status, status)) {
            return;
        }
        if (status.tag === "Closed") {
            this.#autoWifi = Tag("Closed");
        }
        else {
            this.#autoWifi = Tag("Running", {
                controller: current.controller,
                status,
            });
        }
        this.#view.renderAutoWifi(this.#autoWifi);
        this.#recordAutoWifiStatus(status);
    }
    #recordAutoWifiStatus(status) {
        switch (status.tag) {
            case "Starting":
                this.#view.record("Auto Wi-Fi", "Transport starting", null);
                return;
            case "Discovering":
                this.#view.record("Auto Wi-Fi", `Discovery attempt ${status.data.attempt}`, null);
                return;
            case "Active":
                this.#view.record("Auto Wi-Fi", `${status.data.gateways.length} gateway${status.data.gateways.length === 1 ? "" : "s"} active`, status.data.gateways.map((gateway) => gateway.url).join(" · "));
                return;
            case "Unavailable":
                this.#view.record("Failure", "Auto Wi-Fi is unavailable", describeAutoWifiFailure(status.data));
                return;
            case "Closed":
                this.#view.record("Auto Wi-Fi", "Transport closed", null);
                return;
            default:
                this.#view.record("Failure", "Auto Wi-Fi returned an unknown status", describeUnknownOutcome("Auto Wi-Fi status", status));
        }
    }
    #pollUsb() {
        if (this.#usb.tag !== "Connected") {
            return;
        }
        const session = this.#usb.data;
        if (session.status.tag === "Closed") {
            this.#usb = Tag("Closed");
            this.#view.renderUsb(this.#usb);
            this.#view.record("USB Auto", "Session closed by the transport", null);
            return;
        }
        this.#view.renderUsb(this.#usb);
    }
    #pollRuntime() {
        const drained = this.#prns.drainEvents();
        switch (drained.tag) {
            case "Drained":
                for (const event of drained.data) {
                    this.#recordEvent(event);
                }
                break;
            case "RuntimeRejected":
                this.#recordRuntimeFailure("Runtime event drain was rejected", describeRuntimeRejected(drained));
                break;
            default:
                this.#recordRuntimeFailure("Runtime returned an unknown event-drain outcome", describeUnknownOutcome("event drain", drained));
        }
        const captured = this.#prns.snapshot();
        switch (captured.tag) {
            case "Captured":
                this.#snapshot = captured.data;
                this.#view.renderSnapshot(captured.data);
                this.#lastRuntimeFailure = "";
                return;
            case "RuntimeRejected":
                this.#recordRuntimeFailure("Runtime snapshot was rejected", describeRuntimeRejected(captured));
                return;
            default:
                this.#recordRuntimeFailure("Runtime returned an unknown snapshot outcome", describeUnknownOutcome("snapshot", captured));
        }
    }
    #recordEvent(event) {
        switch (event.type) {
            case "announce":
                this.#view.record("Network", "Announce received", `${hex(event.destination)} · ${event.hops} hop${event.hops === 1 ? "" : "s"} · interface ${hex(event.sourceInterface)}`);
                return;
            case "singleDelivery": {
                const metadata = `destination ${hex(event.destination)} · interface ${hex(event.sourceInterface)}`;
                const content = presentPacketContent(event.plaintext);
                switch (content.tag) {
                    case "Empty":
                        this.#view.record("Network", "Single packet received", `${metadata}\n(empty payload)`);
                        return;
                    case "Text":
                        this.#view.record("Network", "Single packet received", `${metadata}\n${content.data.value}`);
                        return;
                    case "Binary":
                        this.#view.record("Network", "Binary single packet received", `${metadata}\n${content.data.byteLength} bytes · ${content.data.hexadecimal}`);
                        return;
                    default:
                        this.#view.record("Failure", "Single packet returned an unknown presentation outcome", describeUnknownOutcome("single packet presentation", content));
                        return;
                }
            }
            case "commandSettled":
                this.#view.record("Announce", `Command ${event.commandId.toString()} settled`, event.debugSettlement);
                return;
            case "routeExpired":
                this.#view.record("Route", "Route expired", hex(event.destination));
                return;
            case "routeEvicted":
                this.#view.record("Route", "Route evicted", hex(event.destination));
                return;
            case "routeInterfaceGone":
                this.#view.record("Route", "Route interface disappeared", hex(event.destination));
                return;
            case "routeDropped":
                this.#view.record("Route", "Route dropped", hex(event.destination));
                return;
            case "unknown":
                this.#view.record("Runtime", "Runtime emitted an event this playground does not recognize", null);
                return;
            default:
                this.#view.record("Failure", "Runtime returned an unknown event", describeUnknownOutcome("runtime event", event));
        }
    }
    #recordRuntimeFailure(summary, detail) {
        const key = `${summary}:${detail}`;
        if (key === this.#lastRuntimeFailure) {
            return;
        }
        this.#lastRuntimeFailure = key;
        this.#view.record("Failure", summary, detail);
    }
    #syncControls() {
        this.#view.setControls(controlAvailability(this.#autoWifi, this.#usb, this.#snapshot));
    }
}
async function boot(document) {
    const binding = bindPlaygroundView(document);
    if (binding.tag !== "Bound") {
        renderBindingFailure(document, binding.data.id);
        return;
    }
    const view = binding.data;
    view.record("Runtime", "Loading the shared Rust engine", null);
    const startup = await BrowserPlayground.start(view);
    if (startup.tag === "Running") {
        return;
    }
    view.renderRuntimeFailure(startup);
    view.renderAutoWifi(Tag("Waiting"));
    view.renderUsb(Tag("Waiting"));
    view.setControls({
        autoWifiStart: false,
        autoWifiClose: false,
        usbConnect: false,
        usbClose: false,
        announce: false,
    });
    view.record("Failure", "Browser node could not start", describeStartupFailure(startup));
}
function usbSession(state) {
    switch (state.tag) {
        case "Connected":
        case "Closing":
            return state.data;
        case "CloseFailed":
            return state.data.session;
        case "Waiting":
        case "Ready":
        case "Unavailable":
        case "Connecting":
        case "ConnectFailed":
        case "Closed":
            return undefined;
        default:
            return unknownState(state);
    }
}
function usbClosableSession(state) {
    switch (state.tag) {
        case "Connected":
            return state.data;
        case "CloseFailed":
            return state.data.session;
        case "Waiting":
        case "Ready":
        case "Unavailable":
        case "Connecting":
        case "Closing":
        case "ConnectFailed":
        case "Closed":
            return undefined;
        default:
            return unknownState(state);
    }
}
function wasmModule() {
    return {
        PrnsRuntime: wasm.PrnsRuntime,
        UsbAutoDecoder: wasm.UsbAutoDecoder,
        BluetoothReassembler: wasm.BluetoothReassembler,
        identitySecretKeyLength: wasm.identitySecretKeyLength,
        bluetoothServiceUuid: wasm.bluetoothServiceUuid,
        bluetoothControlUuid: wasm.bluetoothControlUuid,
        bluetoothDataUuid: wasm.bluetoothDataUuid,
        bluetoothBitrateBps: wasm.bluetoothBitrateBps,
        bluetoothHardwareMtu: wasm.bluetoothHardwareMtu,
        bluetoothDialerHello: wasm.bluetoothDialerHello,
        bluetoothDecodeControl: wasm.bluetoothDecodeControl,
        bluetoothDataFragments: wasm.bluetoothDataFragments,
        websocketBitrateBps: wasm.websocketBitrateBps,
        websocketFrameCap: wasm.websocketFrameCap,
        websocketHardwareMtu: wasm.websocketHardwareMtu,
        usbAutoHostBitrateBps: wasm.usbAutoHostBitrateBps,
        usbAutoHostHardwareMtu: wasm.usbAutoHostHardwareMtu,
        usbAutoWebUsbVendorId: wasm.usbAutoWebUsbVendorId,
        usbAutoWebUsbProductId: wasm.usbAutoWebUsbProductId,
        usbAutoNodeTagFor: wasm.usbAutoNodeTagFor,
        usbAutoHostHelloFrame: wasm.usbAutoHostHelloFrame,
        usbAutoHostHelloAckFrame: wasm.usbAutoHostHelloAckFrame,
        usbAutoDataFrame: wasm.usbAutoDataFrame,
    };
}
function webUsbAvailable() {
    return "usb" in navigator;
}
function unknownState(_state) {
    return undefined;
}
void boot(document);

import init, * as wasm from "./pkg/prns_wasm.js";
import { BrowserLocalStorageIdentityStore, Prns, Tag, match, match_into, } from "./sdk/index.js";
import { BROWSER_PLAYGROUND_LXMF_DELIVERY, LXMF_DELIVERY_DISPLAY_NAME, } from "./lxmf.js";
import { PlaygroundBluetoothController } from "./bluetooth.js";
import { describeAutoWifiFailure, describeCommandFailure, describeHostError, describeHostOperationFailure, describeInterfaceCloseFailure, describeStartupFailure, describeUsbConnectFailure, describeWebSocketConnectFailure, hostOperationFailed, } from "./outcomes.js";
import { hex, presentPacketContent } from "./presentation.js";
import { controlAvailability, sameAutoWifiStatus, webSocketConnectAvailable, } from "./state.js";
import { PlaygroundView, bindPlaygroundView, renderBindingFailure, } from "./view.js";
const POLL_INTERVAL_MS = 250;
const NODE_PAGE_DISPLAY_NAME = "Prns Browser Playground";
const WASM_BINARY_PATH = "./pkg/prns_wasm_bg.wasm";
class BrowserPlayground {
    #view;
    #prns;
    #destination;
    #pageDestination;
    #autoWifi = Tag("Waiting");
    #webSocket = Tag("Waiting");
    #usb = Tag("Waiting");
    #bluetooth;
    #snapshot;
    #pollTimer;
    #lastRuntimeFailure = "";
    #closed = false;
    constructor(view, prns, destination, pageDestination) {
        this.#view = view;
        this.#prns = prns;
        this.#destination = destination;
        this.#pageDestination = pageDestination;
        this.#bluetooth = new PlaygroundBluetoothController(prns.interfaces.bluetooth, view, () => this.#syncControls());
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
            created = await Prns.create({
                wasm: wasmModule(),
                resourceCompressionModuleUrl: new URL("./pkg/prns_wasm.js", globalThis.location.href),
                identityStore: new BrowserLocalStorageIdentityStore(),
            });
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
        const pageRegistered = created.data.registerNodePage(new TextEncoder().encode(NODE_PAGE_DISPLAY_NAME));
        if (pageRegistered.tag !== "Registered") {
            return pageRegistered;
        }
        const playground = new BrowserPlayground(view, created.data, registered.data, pageRegistered.data);
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
        const webSocket = webSocketSession(this.#webSocket);
        const usb = usbSession(this.#usb);
        const autoWifi = this.#autoWifi.tag === "Running"
            ? this.#autoWifi.data.controller
            : undefined;
        this.#usb = Tag("Closed");
        this.#webSocket = Tag("Closed");
        this.#autoWifi = Tag("Closed");
        await Promise.allSettled([
            webSocket?.close(),
            usb?.close(),
            this.#bluetooth.shutdown(),
            autoWifi?.close(),
        ]);
    }
    #run() {
        this.#autoWifi = Tag("Ready");
        this.#webSocket = webSocketAvailable()
            ? Tag("Ready")
            : Tag("Unavailable", { api: "WebSocket" });
        this.#usb = webUsbAvailable()
            ? Tag("Ready")
            : Tag("Unavailable", { api: "WebUSB" });
        this.#bluetooth.start();
        this.#view.renderRuntimeReady(this.#destination);
        this.#view.renderAutoWifi(this.#autoWifi);
        this.#view.renderWebSocket(this.#webSocket);
        this.#view.renderUsb(this.#usb);
        this.#view.record("Runtime", "Browser node runtime ready", `${LXMF_DELIVERY_DISPLAY_NAME} · lxmf.delivery ${hex(this.#destination)}`);
        this.#view.record("Node page", "Serving /page/index.mu and /page/quickstart.mu over Reticulum", `${NODE_PAGE_DISPLAY_NAME} · nomadnetwork.node ${hex(this.#pageDestination)}`);
        this.#view.bindControls({
            startAutoWifi: () => this.#startAutoWifi(),
            closeAutoWifi: () => {
                void this.#closeAutoWifi();
            },
            connectWebSocket: (url) => {
                void this.#connectWebSocket(url);
            },
            closeWebSocket: () => {
                void this.#closeWebSocket();
            },
            connectUsb: () => {
                void this.#connectUsb();
            },
            closeUsb: () => {
                void this.#closeUsb();
            },
            connectBluetooth: () => {
                void this.#bluetooth.connect();
            },
            closeBluetooth: () => {
                void this.#bluetooth.close();
            },
            announce: () => this.#announce(),
            clearActivity: () => this.#view.clearActivity(),
        });
        globalThis.addEventListener("pagehide", () => {
            void this.close();
        });
        const events = this.#prns.claimEvents();
        if (events.tag === "AlreadyClaimed") {
            this.#recordRuntimeFailure("Runtime application event stream unavailable", `${events.data.lane} already has a consumer`);
            return;
        }
        void this.#consumeEvents(events.data);
        const diagnostics = this.#prns.claimDiagnostics();
        if (diagnostics.tag === "AlreadyClaimed") {
            this.#recordRuntimeFailure("Runtime diagnostic stream unavailable", `${diagnostics.data.lane} already has a consumer`);
            return;
        }
        void this.#consumeDiagnostics(diagnostics.data);
        this.#pollTimer = globalThis.setInterval(() => {
            this.#poll();
        }, POLL_INTERVAL_MS);
        this.#poll();
    }
    #startAutoWifi() {
        const available = match_into().from(this.#autoWifi, {
            Waiting: () => false,
            Ready: () => true,
            Running: () => false,
            Closed: () => true,
        });
        if (!available) {
            return;
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
            match(outcome, {
                Closed: () => {
                    this.#autoWifi = Tag("Closed");
                    this.#view.record("Auto Wi-Fi", "Transport closed", null);
                },
                RuntimeRejected: ({ operation, detail }) => {
                    this.#view.record("Failure", "Auto Wi-Fi close was rejected", `${operation}: ${detail}`);
                },
            });
        }
        catch (error) {
            const outcome = hostOperationFailed("Close Auto Wi-Fi", error);
            this.#view.record("Failure", "Auto Wi-Fi close failed", describeHostOperationFailure(outcome));
        }
        this.#pollAutoWifi();
        this.#view.renderAutoWifi(this.#autoWifi);
        this.#syncControls();
    }
    async #connectWebSocket(url) {
        if (!webSocketConnectAvailable(this.#webSocket)) {
            return;
        }
        this.#webSocket = Tag("Connecting", { url });
        this.#view.renderWebSocket(this.#webSocket);
        this.#syncControls();
        this.#view.record("WebSocket", "Opening connection", url || null);
        let outcome;
        try {
            outcome = await this.#prns.interfaces.webSocket.connect(url);
        }
        catch (error) {
            const failure = hostOperationFailed("Connect WebSocket", error);
            this.#webSocket = Tag("ConnectFailed", failure);
            this.#view.renderWebSocket(this.#webSocket);
            this.#view.record("Failure", "WebSocket did not connect", describeHostOperationFailure(failure));
            this.#syncControls();
            return;
        }
        match(outcome, {
            Connected: (session) => {
                this.#webSocket = Tag("Connected", session);
                this.#view.record("WebSocket", "Session opened", `${session.url} · interface ${hex(session.interfaceId)}`);
            },
            HostApiUnavailable: (data) => this.#webSocketConnectFailed(Tag("HostApiUnavailable", data)),
            PermissionDenied: (data) => this.#webSocketConnectFailed(Tag("PermissionDenied", data)),
            Cancelled: (data) => this.#webSocketConnectFailed(Tag("Cancelled", data)),
            AlreadyActive: (data) => this.#webSocketConnectFailed(Tag("AlreadyActive", data)),
            InvalidTarget: (data) => this.#webSocketConnectFailed(Tag("InvalidTarget", data)),
            TimedOut: (data) => this.#webSocketConnectFailed(Tag("TimedOut", data)),
            ConnectionFailed: (data) => this.#webSocketConnectFailed(Tag("ConnectionFailed", data)),
            RuntimeRejected: (data) => this.#webSocketConnectFailed(Tag("RuntimeRejected", data)),
        });
        this.#view.renderWebSocket(this.#webSocket);
        this.#syncControls();
    }
    #webSocketConnectFailed(failure) {
        this.#webSocket = Tag("ConnectFailed", failure);
        this.#view.record("Failure", "WebSocket did not connect", describeWebSocketConnectFailure(failure));
    }
    async #closeWebSocket() {
        const session = webSocketClosableSession(this.#webSocket);
        if (!session) {
            return;
        }
        this.#webSocket = Tag("Closing", session);
        this.#view.renderWebSocket(this.#webSocket);
        this.#syncControls();
        let outcome;
        try {
            outcome = await session.close();
        }
        catch (error) {
            const failure = hostOperationFailed("Close WebSocket", error);
            this.#webSocket = Tag("CloseFailed", { session, failure });
            this.#view.renderWebSocket(this.#webSocket);
            this.#view.record("Failure", "WebSocket close failed", describeHostOperationFailure(failure));
            this.#syncControls();
            return;
        }
        match(outcome, {
            Closed: () => {
                this.#webSocket = Tag("Closed");
                this.#view.record("WebSocket", "Session closed", null);
            },
            CloseFailed: (data) => {
                const failure = Tag("CloseFailed", data);
                this.#webSocket = Tag("CloseFailed", { session, failure });
                this.#view.record("Failure", "WebSocket close failed", describeInterfaceCloseFailure(failure));
            },
        });
        this.#view.renderWebSocket(this.#webSocket);
        this.#syncControls();
    }
    async #connectUsb() {
        const available = match_into().from(this.#usb, {
            Waiting: () => false,
            Ready: () => true,
            Unavailable: () => false,
            Connecting: () => false,
            Connected: () => false,
            Closing: () => false,
            ConnectFailed: () => true,
            Closed: () => true,
            CloseFailed: () => false,
        });
        if (!available) {
            return;
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
        match(outcome, {
            Connected: (session) => {
                this.#usb = Tag("Connected", session);
                this.#view.record("USB Auto", "Session opened", `Interface ${hex(session.interfaceId)}`);
            },
            HostApiUnavailable: (data) => this.#usbConnectFailed(Tag("HostApiUnavailable", data)),
            PermissionDenied: (data) => this.#usbConnectFailed(Tag("PermissionDenied", data)),
            Cancelled: (data) => this.#usbConnectFailed(Tag("Cancelled", data)),
            AlreadyActive: (data) => this.#usbConnectFailed(Tag("AlreadyActive", data)),
            UnsupportedDevice: (data) => this.#usbConnectFailed(Tag("UnsupportedDevice", data)),
            ConnectionFailed: (data) => this.#usbConnectFailed(Tag("ConnectionFailed", data)),
            RuntimeRejected: (data) => this.#usbConnectFailed(Tag("RuntimeRejected", data)),
        });
        this.#view.renderUsb(this.#usb);
        this.#syncControls();
    }
    #usbConnectFailed(failure) {
        this.#usb = Tag("ConnectFailed", failure);
        this.#view.record("Failure", "USB Auto did not connect", describeUsbConnectFailure(failure));
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
        match(outcome, {
            Closed: () => {
                this.#usb = Tag("Closed");
                this.#view.record("USB Auto", "Session closed", null);
            },
            CloseFailed: (data) => {
                const failure = Tag("CloseFailed", data);
                this.#usb = Tag("CloseFailed", { session, failure });
                this.#view.record("Failure", "USB Auto close failed", describeInterfaceCloseFailure(failure));
            },
        });
        this.#view.renderUsb(this.#usb);
        this.#syncControls();
    }
    #announce() {
        if ((this.#snapshot?.interfaces.length ?? 0) === 0) {
            return;
        }
        void this.#announceDestination("LXMF delivery", this.#destination);
        void this.#announceDestination("Node page", this.#pageDestination);
    }
    async #announceDestination(label, destination) {
        const outcome = await this.#prns.announce(destination);
        if (outcome.tag === "Failed") {
            this.#view.record("Failure", `${label} announce failed`, describeCommandFailure(outcome.data));
            return;
        }
        match(outcome.data, {
            Announced: () => {
                this.#view.record("Announce", `${label} announce settled`, null);
            },
        });
    }
    #poll() {
        if (this.#closed) {
            return;
        }
        this.#pollAutoWifi();
        this.#pollWebSocket();
        this.#pollUsb();
        this.#bluetooth.poll();
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
        match(status, {
            Starting: () => {
                this.#view.record("Auto Wi-Fi", "Transport starting", null);
            },
            Discovering: ({ attempt }) => {
                this.#view.record("Auto Wi-Fi", `Discovery attempt ${attempt}`, null);
            },
            Active: ({ gateways }) => {
                this.#view.record("Auto Wi-Fi", `${gateways.length} gateway${gateways.length === 1 ? "" : "s"} active`, gateways.map((gateway) => gateway.url).join(" · "));
            },
            Unavailable: (failure) => {
                this.#view.record("Failure", "Auto Wi-Fi is unavailable", describeAutoWifiFailure(failure));
            },
            Closed: () => {
                this.#view.record("Auto Wi-Fi", "Transport closed", null);
            },
        });
    }
    #pollWebSocket() {
        if (this.#webSocket.tag !== "Connected") {
            return;
        }
        const session = this.#webSocket.data;
        if (session.status.tag === "Closed") {
            this.#webSocket = Tag("Closed");
            this.#view.renderWebSocket(this.#webSocket);
            this.#view.record("WebSocket", "Session closed by the transport", session.url);
            return;
        }
        this.#view.renderWebSocket(this.#webSocket);
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
        const captured = this.#prns.snapshot();
        match(captured, {
            Captured: (snapshot) => {
                this.#snapshot = snapshot;
                this.#view.renderSnapshot(snapshot);
                this.#lastRuntimeFailure = "";
            },
            RuntimeRejected: ({ operation, detail }) => {
                this.#recordRuntimeFailure("Runtime snapshot was rejected", `${operation}: ${detail}`);
            },
        });
    }
    async #consumeEvents(events) {
        try {
            for await (const event of events) {
                if (this.#closed) {
                    return;
                }
                this.#recordEvent(event);
            }
        }
        catch (error) {
            this.#recordRuntimeFailure("Runtime application event stream failed", describeHostError(error));
        }
    }
    async #consumeDiagnostics(diagnostics) {
        try {
            for await (const event of diagnostics) {
                if (this.#closed) {
                    return;
                }
                this.#recordEvent(event);
            }
        }
        catch (error) {
            this.#recordRuntimeFailure("Runtime diagnostic stream failed", describeHostError(error));
        }
    }
    #recordEvent(event) {
        match(event, {
            AnnounceHeard: ({ destination, hops, sourceInterface }) => {
                this.#view.record("Network", "Announce received", `${hex(destination)} · ${hops} hop${hops === 1 ? "" : "s"} · interface ${hex(sourceInterface)}`);
            },
            SingleDelivery: ({ destination, plaintext, sourceInterface }) => {
                const metadata = `destination ${hex(destination)} · interface ${hex(sourceInterface)}`;
                match(presentPacketContent(plaintext), {
                    Empty: () => {
                        this.#view.record("Network", "Single packet received", `${metadata}\n(empty payload)`);
                    },
                    Text: ({ value }) => {
                        this.#view.record("Network", "Single packet received", `${metadata}\n${value}`);
                    },
                    Binary: ({ byteLength, hexadecimal }) => {
                        this.#view.record("Network", "Binary single packet received", `${metadata}\n${byteLength} bytes · ${hexadecimal}`);
                    },
                });
            },
            LinkDelivery: ({ linkId, plaintext, sourceInterface }) => {
                const metadata = `link ${hex(linkId)} · interface ${hex(sourceInterface)}`;
                match(presentPacketContent(plaintext), {
                    Empty: () => {
                        this.#view.record("Network", "Link packet received", `${metadata}\n(empty payload)`);
                    },
                    Text: ({ value }) => {
                        this.#view.record("Network", "Link packet received", `${metadata}\n${value}`);
                    },
                    Binary: ({ byteLength, hexadecimal }) => {
                        this.#view.record("Network", "Binary Link packet received", `${metadata}\n${byteLength} bytes · ${hexadecimal}`);
                    },
                });
            },
            Request: ({ destination, linkId, requestId, data }) => {
                this.#view.record("Network", "Request received", `destination ${hex(destination)} · link ${hex(linkId)} · request ${hex(requestId)} · ${data.length} bytes`);
            },
            Response: ({ linkId, requestId, data }) => {
                this.#view.record("Network", "Response received", `link ${hex(linkId)} · request ${hex(requestId)} · ${data.length} bytes`);
            },
            ResponseSegment: ({ linkId, requestId, segmentIndex, totalSegments, data, }) => {
                this.#view.record("Network", "Response segment received", `link ${hex(linkId)} · request ${hex(requestId)} · segment ${segmentIndex + 1}/${totalSegments} · ${data.length} bytes`);
            },
            ResourceAvailable: ({ linkId, hash, resource }) => {
                this.#view.record("Network", "Resource available", `link ${hex(linkId)} · hash ${hex(hash)} · ${resource.totalBytes} bytes`);
            },
            ResourceSegment: ({ linkId, originalHash, segmentIndex, totalSegments, data, }) => {
                this.#view.record("Network", "Resource segment received", `link ${hex(linkId)} · hash ${hex(originalHash)} · segment ${segmentIndex + 1}/${totalSegments} · ${data.length} bytes`);
            },
            ChannelMessage: ({ linkId, messageType, data }) => {
                this.#view.record("Network", "Channel message received", `link ${hex(linkId)} · ${messageType} · ${data.length} bytes`);
            },
            LinkEstablished: ({ linkId, rttMillis }) => {
                this.#view.record("Network", "Link established", `${hex(linkId)} · ${rttMillis} ms RTT`);
            },
            PeerIdentified: ({ linkId, identity }) => {
                this.#view.record("Network", "Peer identified", `link ${hex(linkId)} · identity ${hex(identity)}`);
            },
            LinkClosed: ({ linkId, reason }) => {
                this.#view.record("Network", "Link closed", `${hex(linkId)} · ${reason}`);
            },
            LinkInterfaceMismatch: ({ linkId, attachedInterface, arrivedOn, }) => {
                this.#view.record("Network", "Packet arrived on the wrong interface", `link ${hex(linkId)} · attached ${hex(attachedInterface)} · arrived ${hex(arrivedOn)}`);
            },
            ResourceNeedsDecompression: ({ linkId, hash, stream, uncompressedDataBytes, }) => {
                this.#view.record("Network", "Resource needs decompression", `link ${hex(linkId)} · hash ${hex(hash)} · ${stream.length} compressed bytes · ${uncompressedDataBytes} uncompressed bytes`);
            },
            ResourceAssembled: ({ linkId, originalHash, totalSizeBytes }) => {
                this.#view.record("Network", "Resource assembled", `link ${hex(linkId)} · hash ${hex(originalHash)} · ${totalSizeBytes} bytes`);
            },
            ResourceFailed: ({ linkId, hash, cause }) => {
                this.#view.record("Failure", "Resource transfer failed", `link ${hex(linkId)} · hash ${hex(hash)} · ${cause}`);
            },
            ResourceSendProgress: ({ linkId, transferredBytes, totalBytes, physicalTransferredBytes, segmentIndex, totalSegments, }) => {
                this.#view.record("Network", "Resource send progress", `link ${hex(linkId)} · ${transferredBytes}/${totalBytes} bytes · ${physicalTransferredBytes} physical bytes · segment ${segmentIndex + 1}/${totalSegments}`);
            },
            SelfRatchetRotated: ({ destination }) => {
                this.#view.record("Runtime", "Self ratchet rotated", hex(destination));
            },
            AnnounceHeldDropped: ({ destination, sourceInterface, cause, }) => {
                this.#view.record("Network", "Held announce dropped", `destination ${hex(destination)} · interface ${hex(sourceInterface)} · ${cause}`);
            },
            Delivered: ({ detail }) => {
                this.#view.record("Network", "Packet delivered", detail);
            },
            BackendDiagnostic: ({ kind, detail }) => {
                this.#view.record("Runtime", kind, detail);
            },
            DiagnosticsDropped: ({ count }) => {
                this.#view.record("Runtime", `${count.toString()} diagnostic event${count === 1n ? "" : "s"} dropped`, null);
            },
            PersistenceRestored: ({ routes, destinationIdentities, tunnels, ratchets }) => {
                this.#view.record("Runtime", "Persistent state restored", `${routes} routes · ${destinationIdentities} identities · ${tunnels} tunnels · ${ratchets} ratchets`);
            },
            PersistenceFlushed: ({ cause, target }) => {
                this.#view.record("Runtime", "Persistent state flushed", `${target} · ${cause}`);
            },
            PersistenceFlushFailed: ({ cause, target }) => {
                this.#view.record("Failure", "Persistent state flush failed", `${target} · ${cause}`);
            },
            RouteExpired: ({ destination }) => {
                this.#view.record("Route", "Route expired", hex(destination));
            },
            RouteEvicted: ({ destination }) => {
                this.#view.record("Route", "Route evicted", hex(destination));
            },
            RouteInterfaceGone: ({ destination }) => {
                this.#view.record("Route", "Route interface disappeared", hex(destination));
            },
            RouteDropped: ({ destination }) => {
                this.#view.record("Route", "Route dropped", hex(destination));
            },
        });
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
        this.#view.setControls(controlAvailability(this.#autoWifi, this.#webSocket, this.#usb, this.#bluetooth.state, this.#snapshot));
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
    view.renderWebSocket(Tag("Waiting"));
    view.renderUsb(Tag("Waiting"));
    view.renderBluetooth(Tag("Waiting"));
    view.setControls({
        autoWifiStart: false,
        autoWifiClose: false,
        webSocketConnect: false,
        webSocketClose: false,
        usbConnect: false,
        usbClose: false,
        bluetoothConnect: false,
        bluetoothClose: false,
        announce: false,
    });
    view.record("Failure", "Browser node could not start", describeStartupFailure(startup));
}
function usbSession(state) {
    return match_into().from(state, {
        Waiting: () => undefined,
        Ready: () => undefined,
        Unavailable: () => undefined,
        Connecting: () => undefined,
        Connected: (session) => session,
        Closing: (session) => session,
        ConnectFailed: () => undefined,
        Closed: () => undefined,
        CloseFailed: ({ session }) => session,
    });
}
function usbClosableSession(state) {
    return match_into().from(state, {
        Waiting: () => undefined,
        Ready: () => undefined,
        Unavailable: () => undefined,
        Connecting: () => undefined,
        Connected: (session) => session,
        Closing: () => undefined,
        ConnectFailed: () => undefined,
        Closed: () => undefined,
        CloseFailed: ({ session }) => session,
    });
}
function webSocketSession(state) {
    return match_into().from(state, {
        Waiting: () => undefined,
        Ready: () => undefined,
        Unavailable: () => undefined,
        Connecting: () => undefined,
        Connected: (session) => session,
        Closing: (session) => session,
        ConnectFailed: () => undefined,
        Closed: () => undefined,
        CloseFailed: ({ session }) => session,
    });
}
function webSocketClosableSession(state) {
    return match_into().from(state, {
        Waiting: () => undefined,
        Ready: () => undefined,
        Unavailable: () => undefined,
        Connecting: () => undefined,
        Connected: (session) => session,
        Closing: () => undefined,
        ConnectFailed: () => undefined,
        Closed: () => undefined,
        CloseFailed: ({ session }) => session,
    });
}
function wasmModule() {
    // wasm-bindgen exposes byte newtypes as Uint8Array; this is the one boundary
    // where the SDK's branded views are attached to those generated bindings.
    return {
        PrnsRuntime: wasm.PrnsRuntime,
        UsbAutoDecoder: wasm.UsbAutoDecoder,
        BluetoothReassembler: wasm.BluetoothReassembler,
        hostContractAbi: wasm.hostContractAbi,
        hostSchemaVersion: wasm.hostSchemaVersion,
        browserPersistenceVersion: wasm.browserPersistenceVersion,
        productVersion: wasm.productVersion,
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
function webSocketAvailable() {
    return typeof globalThis.WebSocket === "function";
}
void boot(document);

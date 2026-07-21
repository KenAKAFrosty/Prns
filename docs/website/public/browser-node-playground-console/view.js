import { Tag } from "./sdk/index.js";
import { describeAutoWifiFailure, describeSessionFailure, describeStartupFailure, describeUnknownOutcome, describeUsbCloseFailure, describeUsbConnectFailure, } from "./outcomes.js";
import { boundedDetail, formatBitrate, hex } from "./presentation.js";
const MAX_ACTIVITY_ENTRIES = 120;
export class PlaygroundView {
    elements;
    constructor(elements) {
        this.elements = elements;
    }
    bindControls(handlers) {
        this.elements.autoWifiStart.addEventListener("click", handlers.startAutoWifi);
        this.elements.autoWifiClose.addEventListener("click", handlers.closeAutoWifi);
        this.elements.usbConnect.addEventListener("click", handlers.connectUsb);
        this.elements.usbClose.addEventListener("click", handlers.closeUsb);
        this.elements.announce.addEventListener("click", handlers.announce);
        this.elements.clearActivity.addEventListener("click", handlers.clearActivity);
    }
    renderRuntimeReady(destination) {
        setStatus(this.elements.runtimeState, "Ready", "active");
        this.elements.destination.textContent = `lxmf.delivery ${hex(destination)}`;
    }
    renderRuntimeFailure(outcome) {
        setStatus(this.elements.runtimeState, "Unavailable", "failed");
        this.elements.destination.textContent = describeStartupFailure(outcome);
    }
    renderAutoWifi(state) {
        switch (state.tag) {
            case "Waiting":
                setStatus(this.elements.autoWifiState, "Waiting", "idle");
                this.elements.autoWifiDetail.textContent = "Waiting for the runtime";
                renderEmpty(this.elements.gatewayList, "No selected gateways yet.");
                return;
            case "Ready":
                setStatus(this.elements.autoWifiState, "Ready", "active");
                this.elements.autoWifiDetail.textContent =
                    "Choose Start Auto Wi-Fi to discover local gateways";
                renderEmpty(this.elements.gatewayList, "Auto Wi-Fi has not been started.");
                return;
            case "Running":
                this.#renderAutoWifiController(state.data.status);
                return;
            case "Closed":
                setStatus(this.elements.autoWifiState, "Closed", "closed");
                this.elements.autoWifiDetail.textContent = "Discovery and sessions stopped";
                renderEmpty(this.elements.gatewayList, "Auto Wi-Fi is closed.");
                return;
            default: {
                const detail = describeUnknownOutcome("Auto Wi-Fi display state", state);
                setStatus(this.elements.autoWifiState, "Protocol mismatch", "failed");
                this.elements.autoWifiDetail.textContent = detail;
                renderEmpty(this.elements.gatewayList, detail);
            }
        }
    }
    renderUsb(status) {
        switch (status.tag) {
            case "Waiting":
                setStatus(this.elements.usbState, "Waiting", "idle");
                this.elements.usbDetail.textContent = "Waiting for the runtime";
                return;
            case "Ready":
                setStatus(this.elements.usbState, "Ready", "active");
                this.elements.usbDetail.textContent =
                    "Choose Connect USB when hardware is attached";
                return;
            case "Unavailable":
                setStatus(this.elements.usbState, "Unavailable", "failed");
                this.elements.usbDetail.textContent = `${status.data.api} is not exposed by this browser`;
                return;
            case "Connecting":
                setStatus(this.elements.usbState, "Selecting device", "working");
                this.elements.usbDetail.textContent = "Complete the browser device prompt";
                return;
            case "Connected":
                this.#renderUsbSession(status.data);
                return;
            case "Closing":
                setStatus(this.elements.usbState, "Closing", "working");
                this.elements.usbDetail.textContent = `interface ${hex(status.data.interfaceId)}`;
                return;
            case "ConnectFailed":
                setStatus(this.elements.usbState, "Not connected", "failed");
                this.elements.usbDetail.textContent = describeUsbConnectFailure(status.data);
                return;
            case "Closed":
                setStatus(this.elements.usbState, "Closed", "closed");
                this.elements.usbDetail.textContent = "The USB transport is closed";
                return;
            case "CloseFailed":
                setStatus(this.elements.usbState, "Close failed", "failed");
                this.elements.usbDetail.textContent = describeUsbCloseFailure(status.data.failure);
                return;
            default: {
                const detail = describeUnknownOutcome("USB display status", status);
                setStatus(this.elements.usbState, "Protocol mismatch", "failed");
                this.elements.usbDetail.textContent = detail;
            }
        }
    }
    renderSnapshot(snapshot) {
        this.elements.interfaceCount.textContent = snapshot.interfaces.length.toString();
        this.elements.routeCount.textContent = snapshot.routes.toString();
        this.elements.packetCount.textContent = snapshot.ingestedPackets.toString();
        this.elements.commandCount.textContent = snapshot.ingestedCommands.toString();
        if (snapshot.interfaces.length === 0) {
            renderEmpty(this.elements.interfaceList, "No interfaces are active.");
            return;
        }
        this.elements.interfaceList.replaceChildren(...snapshot.interfaces.map(renderInterface));
    }
    setControls(availability) {
        this.elements.autoWifiStart.disabled = !availability.autoWifiStart;
        this.elements.autoWifiClose.disabled = !availability.autoWifiClose;
        this.elements.usbConnect.disabled = !availability.usbConnect;
        this.elements.usbClose.disabled = !availability.usbClose;
        this.elements.announce.disabled = !availability.announce;
    }
    record(kind, summary, detail) {
        const item = document.createElement("li");
        item.className = "activity-item";
        const metadata = document.createElement("span");
        metadata.className = "activity-meta";
        metadata.textContent = `${new Date().toLocaleTimeString()}\n${kind}`;
        const message = document.createElement("span");
        message.className = "activity-summary";
        message.textContent = summary;
        if (detail) {
            const detailElement = document.createElement("span");
            detailElement.className = "activity-detail";
            detailElement.textContent = boundedDetail(detail);
            message.append(detailElement);
        }
        item.append(metadata, message);
        this.elements.activityList.prepend(item);
        while (this.elements.activityList.childElementCount > MAX_ACTIVITY_ENTRIES) {
            this.elements.activityList.lastElementChild?.remove();
        }
    }
    clearActivity() {
        this.elements.activityList.replaceChildren();
    }
    #renderAutoWifiController(status) {
        switch (status.tag) {
            case "Starting":
                setStatus(this.elements.autoWifiState, "Starting", "working");
                this.elements.autoWifiDetail.textContent = "Preparing local discovery";
                renderEmpty(this.elements.gatewayList, "Looking for local gateways.");
                return;
            case "Discovering":
                setStatus(this.elements.autoWifiState, "Discovering", "working");
                this.elements.autoWifiDetail.textContent = `attempt ${status.data.attempt}`;
                renderEmpty(this.elements.gatewayList, "Probing localhost and the local network.");
                return;
            case "Active":
                setStatus(this.elements.autoWifiState, "Active", "active");
                this.elements.autoWifiDetail.textContent = `${status.data.gateways.length} selected gateway${status.data.gateways.length === 1 ? "" : "s"}`;
                this.elements.gatewayList.replaceChildren(...status.data.gateways.map((gateway) => dataCard(gateway.localhost ? "Localhost gateway" : "LAN gateway", [
                    ["id", gateway.id],
                    ["url", gateway.url],
                    ["interface", hex(gateway.interfaceId)],
                ])));
                return;
            case "Unavailable":
                setStatus(this.elements.autoWifiState, "Unavailable", "failed");
                this.elements.autoWifiDetail.textContent = describeAutoWifiFailure(status.data);
                renderEmpty(this.elements.gatewayList, "No gateway is currently attached. Discovery will retry within its bounds.");
                return;
            case "Closed":
                setStatus(this.elements.autoWifiState, "Closed", "closed");
                this.elements.autoWifiDetail.textContent = "Discovery and sessions stopped";
                renderEmpty(this.elements.gatewayList, "Auto Wi-Fi is closed.");
                return;
            default: {
                const detail = describeUnknownOutcome("Auto Wi-Fi status", status);
                setStatus(this.elements.autoWifiState, "Protocol mismatch", "failed");
                this.elements.autoWifiDetail.textContent = detail;
                renderEmpty(this.elements.gatewayList, detail);
            }
        }
    }
    #renderUsbSession(session) {
        const interfaceId = hex(session.interfaceId);
        switch (session.status.tag) {
            case "Negotiating":
                setStatus(this.elements.usbState, "Negotiating", "working");
                this.elements.usbDetail.textContent = `interface ${interfaceId}`;
                return;
            case "Active":
                setStatus(this.elements.usbState, "Active", "active");
                this.elements.usbDetail.textContent = `interface ${interfaceId}`;
                return;
            case "Closed":
                setStatus(this.elements.usbState, "Closed", "closed");
                this.elements.usbDetail.textContent = `interface ${interfaceId}`;
                return;
            case "Failed":
                setStatus(this.elements.usbState, "Session failed", "failed");
                this.elements.usbDetail.textContent = describeSessionFailure(session.status.data);
                return;
            default: {
                const detail = describeUnknownOutcome("USB session status", session.status);
                setStatus(this.elements.usbState, "Protocol mismatch", "failed");
                this.elements.usbDetail.textContent = detail;
            }
        }
    }
}
export function bindPlaygroundView(document) {
    const runtimeState = document.getElementById("runtime-state");
    if (!(runtimeState instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "runtime-state" });
    }
    const destination = document.getElementById("destination");
    if (!(destination instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "destination" });
    }
    const autoWifiState = document.getElementById("auto-wifi-state");
    if (!(autoWifiState instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "auto-wifi-state" });
    }
    const autoWifiDetail = document.getElementById("auto-wifi-detail");
    if (!(autoWifiDetail instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "auto-wifi-detail" });
    }
    const usbState = document.getElementById("usb-state");
    if (!(usbState instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "usb-state" });
    }
    const usbDetail = document.getElementById("usb-detail");
    if (!(usbDetail instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "usb-detail" });
    }
    const interfaceCount = document.getElementById("interface-count");
    if (!(interfaceCount instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "interface-count" });
    }
    const routeCount = document.getElementById("route-count");
    if (!(routeCount instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "route-count" });
    }
    const packetCount = document.getElementById("packet-count");
    if (!(packetCount instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "packet-count" });
    }
    const commandCount = document.getElementById("command-count");
    if (!(commandCount instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "command-count" });
    }
    const gatewayList = document.getElementById("gateway-list");
    if (!(gatewayList instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "gateway-list" });
    }
    const interfaceList = document.getElementById("interface-list");
    if (!(interfaceList instanceof HTMLElement)) {
        return Tag("MissingElement", { id: "interface-list" });
    }
    const activityList = document.getElementById("activity-list");
    if (!(activityList instanceof HTMLOListElement)) {
        return Tag("MissingElement", { id: "activity-list" });
    }
    const autoWifiStart = document.getElementById("wifi-start");
    if (!(autoWifiStart instanceof HTMLButtonElement)) {
        return Tag("MissingElement", { id: "wifi-start" });
    }
    const autoWifiClose = document.getElementById("wifi-close");
    if (!(autoWifiClose instanceof HTMLButtonElement)) {
        return Tag("MissingElement", { id: "wifi-close" });
    }
    const usbConnect = document.getElementById("usb-connect");
    if (!(usbConnect instanceof HTMLButtonElement)) {
        return Tag("MissingElement", { id: "usb-connect" });
    }
    const usbClose = document.getElementById("usb-close");
    if (!(usbClose instanceof HTMLButtonElement)) {
        return Tag("MissingElement", { id: "usb-close" });
    }
    const announce = document.getElementById("announce");
    if (!(announce instanceof HTMLButtonElement)) {
        return Tag("MissingElement", { id: "announce" });
    }
    const clearActivity = document.getElementById("clear-activity");
    if (!(clearActivity instanceof HTMLButtonElement)) {
        return Tag("MissingElement", { id: "clear-activity" });
    }
    return Tag("Bound", new PlaygroundView({
        runtimeState,
        destination,
        autoWifiState,
        autoWifiDetail,
        usbState,
        usbDetail,
        interfaceCount,
        routeCount,
        packetCount,
        commandCount,
        gatewayList,
        interfaceList,
        activityList,
        autoWifiStart,
        autoWifiClose,
        usbConnect,
        usbClose,
        announce,
        clearActivity,
    }));
}
export function renderBindingFailure(document, id) {
    const main = document.createElement("main");
    const heading = document.createElement("h1");
    heading.textContent = "Playground markup mismatch";
    const detail = document.createElement("p");
    detail.textContent = `The required ${id} element is unavailable.`;
    main.append(heading, detail);
    document.body?.replaceChildren(main);
}
function renderInterface(snapshot) {
    return dataCard(snapshot.kind, [
        ["id", hex(snapshot.id)],
        ["routes", snapshot.routes.toString()],
        ["links", snapshot.links.toString()],
        ["bitrate", formatBitrate(snapshot.bitrateBps)],
        ["mtu", snapshot.hardwareMtu?.toString() ?? "unknown"],
    ]);
}
function dataCard(title, values) {
    const card = document.createElement("div");
    card.className = "data-card";
    const heading = document.createElement("strong");
    heading.textContent = title;
    const list = document.createElement("dl");
    for (const [label, value] of values) {
        const term = document.createElement("dt");
        term.textContent = label;
        const detail = document.createElement("dd");
        detail.textContent = value;
        list.append(term, detail);
    }
    card.append(heading, list);
    return card;
}
function renderEmpty(container, message) {
    const empty = document.createElement("div");
    empty.className = "empty-card";
    empty.textContent = message;
    container.replaceChildren(empty);
}
function setStatus(element, label, state) {
    element.textContent = label;
    element.dataset.state = state;
}

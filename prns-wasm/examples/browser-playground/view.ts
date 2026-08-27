import { Tag, match } from "./sdk/index.js";
import type {
  AutoWifiControllerStatus,
  BluetoothSession,
  DestinationHash,
  InterfaceSnapshot,
  PrnsSnapshot,
  Tag as Tagged,
  UsbAutoSession,
  WebSocketSession,
} from "./sdk/index.js";
import {
  describeAutoWifiFailure,
  describeBluetoothConnectFailure,
  describeInterfaceCloseFailure,
  describeSessionFailure,
  describeStartupFailure,
  describeUsbConnectFailure,
  describeWebSocketConnectFailure,
} from "./outcomes.js";
import type { StartupFailure } from "./outcomes.js";
import {
  boundedDetail,
  describeBluetoothUnavailable,
  formatBitrate,
  hex,
} from "./presentation.js";
import type {
  AutoWifiState,
  BluetoothState,
  ControlAvailability,
  UsbState,
  WebSocketState,
} from "./state.js";

const MAX_ACTIVITY_ENTRIES = 120;

export type ActivityKind =
  | "Runtime"
  | "Auto Wi-Fi"
  | "WebSocket"
  | "USB Auto"
  | "Bluetooth"
  | "Announce"
  | "Node page"
  | "Network"
  | "Route"
  | "Failure";

export type PlaygroundControlHandlers = {
  readonly startAutoWifi: () => void;
  readonly closeAutoWifi: () => void;
  readonly connectWebSocket: (url: string) => void;
  readonly closeWebSocket: () => void;
  readonly connectUsb: () => void;
  readonly closeUsb: () => void;
  readonly connectBluetooth: () => void;
  readonly closeBluetooth: () => void;
  readonly announce: () => void;
  readonly clearActivity: () => void;
};

type DomBindingOutcome =
  | Tagged<"Bound", PlaygroundView>
  | Tagged<"MissingElement", { readonly id: string }>;

type PlaygroundElements = {
  readonly runtimeState: HTMLElement;
  readonly destination: HTMLElement;
  readonly autoWifiState: HTMLElement;
  readonly autoWifiDetail: HTMLElement;
  readonly webSocketState: HTMLElement;
  readonly webSocketDetail: HTMLElement;
  readonly usbState: HTMLElement;
  readonly usbDetail: HTMLElement;
  readonly bluetoothState: HTMLElement;
  readonly bluetoothDetail: HTMLElement;
  readonly interfaceCount: HTMLElement;
  readonly routeCount: HTMLElement;
  readonly packetCount: HTMLElement;
  readonly commandCount: HTMLElement;
  readonly gatewayList: HTMLElement;
  readonly interfaceList: HTMLElement;
  readonly activityList: HTMLOListElement;
  readonly autoWifiStart: HTMLButtonElement;
  readonly autoWifiClose: HTMLButtonElement;
  readonly webSocketForm: HTMLFormElement;
  readonly webSocketUrl: HTMLInputElement;
  readonly webSocketConnect: HTMLButtonElement;
  readonly webSocketClose: HTMLButtonElement;
  readonly usbConnect: HTMLButtonElement;
  readonly usbClose: HTMLButtonElement;
  readonly bluetoothConnect: HTMLButtonElement;
  readonly bluetoothClose: HTMLButtonElement;
  readonly announce: HTMLButtonElement;
  readonly clearActivity: HTMLButtonElement;
};

export class PlaygroundView {
  private readonly elements: PlaygroundElements;

  constructor(elements: PlaygroundElements) {
    this.elements = elements;
  }

  bindControls(handlers: PlaygroundControlHandlers): void {
    this.elements.autoWifiStart.addEventListener(
      "click",
      handlers.startAutoWifi,
    );
    this.elements.autoWifiClose.addEventListener(
      "click",
      handlers.closeAutoWifi,
    );
    this.elements.webSocketForm.addEventListener("submit", (event) => {
      event.preventDefault();
      handlers.connectWebSocket(this.elements.webSocketUrl.value.trim());
    });
    this.elements.webSocketClose.addEventListener(
      "click",
      handlers.closeWebSocket,
    );
    this.elements.usbConnect.addEventListener("click", handlers.connectUsb);
    this.elements.usbClose.addEventListener("click", handlers.closeUsb);
    this.elements.bluetoothConnect.addEventListener(
      "click",
      handlers.connectBluetooth,
    );
    this.elements.bluetoothClose.addEventListener(
      "click",
      handlers.closeBluetooth,
    );
    this.elements.announce.addEventListener("click", handlers.announce);
    this.elements.clearActivity.addEventListener(
      "click",
      handlers.clearActivity,
    );
  }

  renderRuntimeReady(destination: DestinationHash): void {
    setStatus(this.elements.runtimeState, "Ready", "active");
    this.elements.destination.textContent = `lxmf.delivery ${hex(destination)}`;
  }

  renderRuntimeFailure(outcome: StartupFailure): void {
    setStatus(this.elements.runtimeState, "Unavailable", "failed");
    this.elements.destination.textContent = describeStartupFailure(outcome);
  }

  renderAutoWifi(state: AutoWifiState): void {
    match(state, {
      Waiting: () => {
        setStatus(this.elements.autoWifiState, "Waiting", "idle");
        this.elements.autoWifiDetail.textContent = "Waiting for the runtime";
        renderEmpty(this.elements.gatewayList, "No selected gateways yet.");
      },
      Ready: () => {
        setStatus(this.elements.autoWifiState, "Ready", "active");
        this.elements.autoWifiDetail.textContent =
          "Choose Start Auto Wi-Fi to discover local gateways";
        renderEmpty(
          this.elements.gatewayList,
          "Auto Wi-Fi has not been started.",
        );
      },
      Running: ({ status }) => {
        this.#renderAutoWifiController(status);
      },
      Closed: () => {
        setStatus(this.elements.autoWifiState, "Closed", "closed");
        this.elements.autoWifiDetail.textContent = "Discovery and sessions stopped";
        renderEmpty(this.elements.gatewayList, "Auto Wi-Fi is closed.");
      },
    });
  }

  renderUsb(status: UsbState): void {
    match(status, {
      Waiting: () => {
        setStatus(this.elements.usbState, "Waiting", "idle");
        this.elements.usbDetail.textContent = "Waiting for the runtime";
      },
      Ready: () => {
        setStatus(this.elements.usbState, "Ready", "active");
        this.elements.usbDetail.textContent =
          "Choose Connect USB when a Hopspot is attached";
      },
      Unavailable: ({ api }) => {
        setStatus(this.elements.usbState, "Unavailable", "failed");
        this.elements.usbDetail.textContent = `${api} is not exposed by this browser`;
      },
      Connecting: () => {
        setStatus(this.elements.usbState, "Selecting device", "working");
        this.elements.usbDetail.textContent = "Complete the browser device prompt";
      },
      Connected: (session) => {
        this.#renderUsbSession(session);
      },
      Closing: (session) => {
        setStatus(this.elements.usbState, "Closing", "working");
        this.elements.usbDetail.textContent = `interface ${hex(session.interfaceId)}`;
      },
      ConnectFailed: (failure) => {
        setStatus(this.elements.usbState, "Not connected", "failed");
        this.elements.usbDetail.textContent =
          describeUsbConnectFailure(failure);
      },
      Closed: () => {
        setStatus(this.elements.usbState, "Closed", "closed");
        this.elements.usbDetail.textContent = "The USB transport is closed";
      },
      CloseFailed: ({ failure }) => {
        setStatus(this.elements.usbState, "Close failed", "failed");
        this.elements.usbDetail.textContent =
          describeInterfaceCloseFailure(failure);
      },
    });
  }

  renderBluetooth(status: BluetoothState): void {
    match(status, {
      Waiting: () => {
        setStatus(this.elements.bluetoothState, "Waiting", "idle");
        this.elements.bluetoothDetail.textContent = "Waiting for the runtime";
      },
      Ready: () => {
        setStatus(this.elements.bluetoothState, "Ready", "active");
        this.elements.bluetoothDetail.textContent =
          "Choose Connect Bluetooth near an advertising Prns node";
      },
      Unavailable: () => {
        setStatus(this.elements.bluetoothState, "Unavailable", "failed");
        this.elements.bluetoothDetail.textContent =
          describeBluetoothUnavailable(navigator);
      },
      Connecting: () => {
        setStatus(this.elements.bluetoothState, "Selecting device", "working");
        this.elements.bluetoothDetail.textContent =
          "Choose a Prns node in the browser device prompt";
      },
      Session: (session) => {
        this.#renderBluetoothSession(session);
      },
      SessionFailed: (failure) => {
        setStatus(this.elements.bluetoothState, "Session failed", "failed");
        this.elements.bluetoothDetail.textContent =
          describeSessionFailure(failure);
      },
      Closing: (session) => {
        setStatus(this.elements.bluetoothState, "Closing", "working");
        this.elements.bluetoothDetail.textContent =
          `interface ${hex(session.interfaceId)}`;
      },
      ConnectFailed: (failure) => {
        setStatus(this.elements.bluetoothState, "Not connected", "failed");
        this.elements.bluetoothDetail.textContent =
          describeBluetoothConnectFailure(failure);
      },
      Closed: () => {
        setStatus(this.elements.bluetoothState, "Closed", "closed");
        this.elements.bluetoothDetail.textContent =
          "The Bluetooth transport is closed";
      },
      CloseFailed: ({ failure }) => {
        setStatus(this.elements.bluetoothState, "Close failed", "failed");
        this.elements.bluetoothDetail.textContent =
          describeInterfaceCloseFailure(failure);
      },
    });
  }

  renderWebSocket(status: WebSocketState): void {
    match(status, {
      Waiting: () => {
        setStatus(this.elements.webSocketState, "Waiting", "idle");
        this.elements.webSocketDetail.textContent = "Waiting for the runtime";
      },
      Ready: () => {
        setStatus(this.elements.webSocketState, "Ready", "active");
        this.elements.webSocketDetail.textContent =
          "Enter a ws:// or wss:// Prns endpoint";
      },
      Unavailable: ({ api }) => {
        setStatus(this.elements.webSocketState, "Unavailable", "failed");
        this.elements.webSocketDetail.textContent =
          `${api} is not exposed by this browser`;
      },
      Connecting: ({ url }) => {
        setStatus(this.elements.webSocketState, "Connecting", "working");
        this.elements.webSocketDetail.textContent = url || "No URL provided";
      },
      Connected: (session) => {
        this.#renderWebSocketSession(session);
      },
      Closing: (session) => {
        setStatus(this.elements.webSocketState, "Closing", "working");
        this.elements.webSocketDetail.textContent = session.url;
      },
      ConnectFailed: (failure) => {
        setStatus(this.elements.webSocketState, "Not connected", "failed");
        this.elements.webSocketDetail.textContent =
          describeWebSocketConnectFailure(failure);
      },
      Closed: () => {
        setStatus(this.elements.webSocketState, "Closed", "closed");
        this.elements.webSocketDetail.textContent =
          "The WebSocket transport is closed";
      },
      CloseFailed: ({ failure }) => {
        setStatus(this.elements.webSocketState, "Close failed", "failed");
        this.elements.webSocketDetail.textContent =
          describeInterfaceCloseFailure(failure);
      },
    });
  }

  renderSnapshot(snapshot: PrnsSnapshot): void {
    this.elements.interfaceCount.textContent = snapshot.interfaces.length.toString();
    this.elements.routeCount.textContent = snapshot.routes.toString();
    this.elements.packetCount.textContent = snapshot.ingestedPackets.toString();
    this.elements.commandCount.textContent = snapshot.ingestedCommands.toString();
    if (snapshot.interfaces.length === 0) {
      renderEmpty(this.elements.interfaceList, "No interfaces are active.");
      return;
    }
    this.elements.interfaceList.replaceChildren(
      ...snapshot.interfaces.map(renderInterface),
    );
  }

  setControls(availability: ControlAvailability): void {
    this.elements.autoWifiStart.disabled = !availability.autoWifiStart;
    this.elements.autoWifiClose.disabled = !availability.autoWifiClose;
    this.elements.webSocketConnect.disabled =
      !availability.webSocketConnect;
    this.elements.webSocketClose.disabled = !availability.webSocketClose;
    this.elements.webSocketUrl.readOnly = !availability.webSocketConnect;
    this.elements.usbConnect.disabled = !availability.usbConnect;
    this.elements.usbClose.disabled = !availability.usbClose;
    this.elements.bluetoothConnect.disabled =
      !availability.bluetoothConnect;
    this.elements.bluetoothClose.disabled = !availability.bluetoothClose;
    this.elements.announce.disabled = !availability.announce;
  }

  record(kind: ActivityKind, summary: string, detail: string | null): void {
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

  clearActivity(): void {
    this.elements.activityList.replaceChildren();
  }

  #renderAutoWifiController(status: AutoWifiControllerStatus): void {
    match(status, {
      Starting: () => {
        setStatus(this.elements.autoWifiState, "Starting", "working");
        this.elements.autoWifiDetail.textContent = "Preparing local discovery";
        renderEmpty(this.elements.gatewayList, "Looking for local gateways.");
      },
      Discovering: ({ attempt }) => {
        setStatus(this.elements.autoWifiState, "Discovering", "working");
        this.elements.autoWifiDetail.textContent = `attempt ${attempt}`;
        renderEmpty(this.elements.gatewayList, "Probing localhost and the local network.");
      },
      Active: ({ gateways }) => {
        setStatus(this.elements.autoWifiState, "Active", "active");
        this.elements.autoWifiDetail.textContent = `${gateways.length} selected gateway${gateways.length === 1 ? "" : "s"}`;
        this.elements.gatewayList.replaceChildren(
          ...gateways.map((gateway) =>
            dataCard(gateway.localhost ? "Localhost gateway" : "LAN gateway", [
              ["id", gateway.id],
              ["url", gateway.url],
              ["interface", hex(gateway.interfaceId)],
            ]),
          ),
        );
      },
      Unavailable: (failure) => {
        setStatus(this.elements.autoWifiState, "Unavailable", "failed");
        this.elements.autoWifiDetail.textContent =
          describeAutoWifiFailure(failure);
        renderEmpty(
          this.elements.gatewayList,
          "No gateway is currently attached. Discovery will retry within its bounds.",
        );
      },
      Closed: () => {
        setStatus(this.elements.autoWifiState, "Closed", "closed");
        this.elements.autoWifiDetail.textContent = "Discovery and sessions stopped";
        renderEmpty(this.elements.gatewayList, "Auto Wi-Fi is closed.");
      },
    });
  }

  #renderUsbSession(session: UsbAutoSession): void {
    const interfaceId = hex(session.interfaceId);
    match(session.status, {
      Negotiating: () => {
        setStatus(this.elements.usbState, "Negotiating", "working");
        this.elements.usbDetail.textContent = `interface ${interfaceId}`;
      },
      Active: () => {
        setStatus(this.elements.usbState, "Active", "active");
        this.elements.usbDetail.textContent = `interface ${interfaceId}`;
      },
      Closed: () => {
        setStatus(this.elements.usbState, "Closed", "closed");
        this.elements.usbDetail.textContent = `interface ${interfaceId}`;
      },
      Failed: (failure) => {
        setStatus(this.elements.usbState, "Session failed", "failed");
        this.elements.usbDetail.textContent =
          describeSessionFailure(failure);
      },
    });
  }

  #renderBluetoothSession(session: BluetoothSession): void {
    const interfaceId = hex(session.interfaceId);
    match(session.status, {
      Negotiating: () => {
        setStatus(this.elements.bluetoothState, "Negotiating", "working");
        this.elements.bluetoothDetail.textContent = `interface ${interfaceId}`;
      },
      Active: () => {
        setStatus(this.elements.bluetoothState, "Active", "active");
        this.elements.bluetoothDetail.textContent = `interface ${interfaceId}`;
      },
      Closed: () => {
        setStatus(this.elements.bluetoothState, "Closed", "closed");
        this.elements.bluetoothDetail.textContent = `interface ${interfaceId}`;
      },
      Failed: (failure) => {
        setStatus(this.elements.bluetoothState, "Session failed", "failed");
        this.elements.bluetoothDetail.textContent =
          describeSessionFailure(failure);
      },
    });
  }

  #renderWebSocketSession(session: WebSocketSession): void {
    match(session.status, {
      Negotiating: () => {
        setStatus(this.elements.webSocketState, "Negotiating", "working");
        this.elements.webSocketDetail.textContent = session.url;
      },
      Active: () => {
        setStatus(this.elements.webSocketState, "Active", "active");
        this.elements.webSocketDetail.textContent = session.url;
      },
      Closed: () => {
        setStatus(this.elements.webSocketState, "Closed", "closed");
        this.elements.webSocketDetail.textContent = session.url;
      },
      Failed: (failure) => {
        setStatus(this.elements.webSocketState, "Session failed", "failed");
        this.elements.webSocketDetail.textContent =
          describeSessionFailure(failure);
      },
    });
  }
}

export function bindPlaygroundView(document: Document): DomBindingOutcome {
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
  const webSocketState = document.getElementById("websocket-state");
  if (!(webSocketState instanceof HTMLElement)) {
    return Tag("MissingElement", { id: "websocket-state" });
  }
  const webSocketDetail = document.getElementById("websocket-detail");
  if (!(webSocketDetail instanceof HTMLElement)) {
    return Tag("MissingElement", { id: "websocket-detail" });
  }
  const usbState = document.getElementById("usb-state");
  if (!(usbState instanceof HTMLElement)) {
    return Tag("MissingElement", { id: "usb-state" });
  }
  const usbDetail = document.getElementById("usb-detail");
  if (!(usbDetail instanceof HTMLElement)) {
    return Tag("MissingElement", { id: "usb-detail" });
  }
  const bluetoothState = document.getElementById("bluetooth-state");
  if (!(bluetoothState instanceof HTMLElement)) {
    return Tag("MissingElement", { id: "bluetooth-state" });
  }
  const bluetoothDetail = document.getElementById("bluetooth-detail");
  if (!(bluetoothDetail instanceof HTMLElement)) {
    return Tag("MissingElement", { id: "bluetooth-detail" });
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
  const webSocketForm = document.getElementById("websocket-form");
  if (!(webSocketForm instanceof HTMLFormElement)) {
    return Tag("MissingElement", { id: "websocket-form" });
  }
  const webSocketUrl = document.getElementById("websocket-url");
  if (!(webSocketUrl instanceof HTMLInputElement)) {
    return Tag("MissingElement", { id: "websocket-url" });
  }
  const webSocketConnect = document.getElementById("websocket-connect");
  if (!(webSocketConnect instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "websocket-connect" });
  }
  const webSocketClose = document.getElementById("websocket-close");
  if (!(webSocketClose instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "websocket-close" });
  }
  const usbConnect = document.getElementById("usb-connect");
  if (!(usbConnect instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "usb-connect" });
  }
  const usbClose = document.getElementById("usb-close");
  if (!(usbClose instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "usb-close" });
  }
  const bluetoothConnect = document.getElementById("bluetooth-connect");
  if (!(bluetoothConnect instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "bluetooth-connect" });
  }
  const bluetoothClose = document.getElementById("bluetooth-close");
  if (!(bluetoothClose instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "bluetooth-close" });
  }
  const announce = document.getElementById("announce");
  if (!(announce instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "announce" });
  }
  const clearActivity = document.getElementById("clear-activity");
  if (!(clearActivity instanceof HTMLButtonElement)) {
    return Tag("MissingElement", { id: "clear-activity" });
  }
  return Tag(
    "Bound",
    new PlaygroundView({
      runtimeState,
      destination,
      autoWifiState,
      autoWifiDetail,
      webSocketState,
      webSocketDetail,
      usbState,
      usbDetail,
      bluetoothState,
      bluetoothDetail,
      interfaceCount,
      routeCount,
      packetCount,
      commandCount,
      gatewayList,
      interfaceList,
      activityList,
      autoWifiStart,
      autoWifiClose,
      webSocketForm,
      webSocketUrl,
      webSocketConnect,
      webSocketClose,
      usbConnect,
      usbClose,
      bluetoothConnect,
      bluetoothClose,
      announce,
      clearActivity,
    }),
  );
}

export function renderBindingFailure(document: Document, id: string): void {
  const main = document.createElement("main");
  const heading = document.createElement("h1");
  heading.textContent = "Playground markup mismatch";
  const detail = document.createElement("p");
  detail.textContent = `The required ${id} element is unavailable.`;
  main.append(heading, detail);
  document.body?.replaceChildren(main);
}

function renderInterface(snapshot: InterfaceSnapshot): HTMLElement {
  return dataCard(snapshot.kind, [
    ["id", hex(snapshot.id)],
    ["routes", snapshot.routes.toString()],
    ["links", snapshot.links.toString()],
    ["bitrate", formatBitrate(snapshot.bitrateBps)],
    ["mtu", snapshot.hardwareMtu?.toString() ?? "unknown"],
  ]);
}

function dataCard(
  title: string,
  values: readonly (readonly [string, string])[],
): HTMLElement {
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

function renderEmpty(container: HTMLElement, message: string): void {
  const empty = document.createElement("div");
  empty.className = "empty-card";
  empty.textContent = message;
  container.replaceChildren(empty);
}

function setStatus(
  element: HTMLElement,
  label: string,
  state: "idle" | "working" | "active" | "failed" | "closed",
): void {
  element.textContent = label;
  element.dataset.state = state;
}

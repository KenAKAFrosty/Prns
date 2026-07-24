import { Tag, match } from "./sdk/index.js";
import type {
  AutoWifiControllerStatus,
  DestinationHash,
  InterfaceSnapshot,
  PrnsSnapshot,
  Tag as Tagged,
  UsbAutoSession,
} from "./sdk/index.js";
import {
  describeAutoWifiFailure,
  describeSessionFailure,
  describeStartupFailure,
  describeUsbCloseFailure,
  describeUsbConnectFailure,
} from "./outcomes.js";
import type { StartupFailure } from "./outcomes.js";
import { boundedDetail, formatBitrate, hex } from "./presentation.js";
import type {
  AutoWifiState,
  ControlAvailability,
  UsbState,
} from "./state.js";

const MAX_ACTIVITY_ENTRIES = 120;

export type ActivityKind =
  | "Runtime"
  | "Auto Wi-Fi"
  | "USB Auto"
  | "Announce"
  | "Node page"
  | "Network"
  | "Route"
  | "Failure";

export type PlaygroundControlHandlers = {
  readonly startAutoWifi: () => void;
  readonly closeAutoWifi: () => void;
  readonly connectUsb: () => void;
  readonly closeUsb: () => void;
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
  readonly usbState: HTMLElement;
  readonly usbDetail: HTMLElement;
  readonly interfaceCount: HTMLElement;
  readonly routeCount: HTMLElement;
  readonly packetCount: HTMLElement;
  readonly commandCount: HTMLElement;
  readonly gatewayList: HTMLElement;
  readonly interfaceList: HTMLElement;
  readonly activityList: HTMLOListElement;
  readonly autoWifiStart: HTMLButtonElement;
  readonly autoWifiClose: HTMLButtonElement;
  readonly usbConnect: HTMLButtonElement;
  readonly usbClose: HTMLButtonElement;
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
    this.elements.usbConnect.addEventListener("click", handlers.connectUsb);
    this.elements.usbClose.addEventListener("click", handlers.closeUsb);
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
          describeUsbCloseFailure(failure);
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
    this.elements.usbConnect.disabled = !availability.usbConnect;
    this.elements.usbClose.disabled = !availability.usbClose;
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
  return Tag(
    "Bound",
    new PlaygroundView({
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

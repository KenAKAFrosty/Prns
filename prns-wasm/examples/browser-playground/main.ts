import init, * as wasm from "./pkg/prns_wasm.js";
import {
  BrowserLocalStorageIdentityStore,
  Prns,
  Tag,
  match,
} from "./sdk/index.js";
import type {
  AutoWifiControllerStatus,
  DestinationHash,
  InterfaceCloseOutcome,
  PrnsCreateOutcome,
  PrnsEvent,
  PrnsSnapshot,
  PrnsWasmModule,
  Tag as Tagged,
  UsbAutoConnectOutcome,
  UsbAutoSession,
} from "./sdk/index.js";
import {
  BROWSER_PLAYGROUND_LXMF_DELIVERY,
  LXMF_DELIVERY_DISPLAY_NAME,
} from "./lxmf.js";
import {
  describeAutoWifiFailure,
  describeHostError,
  describeHostOperationFailure,
  describeRuntimeRejected,
  describeStartupFailure,
  describeUnknownOutcome,
  describeUsbCloseFailure,
  describeUsbConnectFailure,
  hostOperationFailed,
} from "./outcomes.js";
import type { StartupFailure } from "./outcomes.js";
import { hex, presentPacketContent } from "./presentation.js";
import {
  controlAvailability,
  sameAutoWifiStatus,
} from "./state.js";
import type { AutoWifiState, UsbState } from "./state.js";
import {
  PlaygroundView,
  bindPlaygroundView,
  renderBindingFailure,
} from "./view.js";

const POLL_INTERVAL_MS = 250;
const NODE_PAGE_DISPLAY_NAME = "Prns Browser Playground";
const WASM_BINARY_PATH = "./pkg/prns_wasm_bg.wasm";

type StartupOutcome = Tagged<"Running", BrowserPlayground> | StartupFailure;

class BrowserPlayground {
  readonly #view: PlaygroundView;
  readonly #prns: Prns;
  readonly #destination: DestinationHash;
  readonly #pageDestination: DestinationHash;
  #autoWifi: AutoWifiState = Tag("Waiting");
  #usb: UsbState = Tag("Waiting");
  #snapshot: PrnsSnapshot | undefined;
  #pollTimer: number | undefined;
  #lastRuntimeFailure = "";
  #closed = false;

  private constructor(
    view: PlaygroundView,
    prns: Prns,
    destination: DestinationHash,
    pageDestination: DestinationHash,
  ) {
    this.#view = view;
    this.#prns = prns;
    this.#destination = destination;
    this.#pageDestination = pageDestination;
  }

  static async start(view: PlaygroundView): Promise<StartupOutcome> {
    if (BROWSER_PLAYGROUND_LXMF_DELIVERY.tag !== "Prepared") {
      return BROWSER_PLAYGROUND_LXMF_DELIVERY;
    }
    try {
      await init({
        module_or_path: new URL(WASM_BINARY_PATH, globalThis.location.href),
      });
    } catch (error: unknown) {
      return Tag("WasmLoadFailed", { detail: describeHostError(error) });
    }
    let created: PrnsCreateOutcome;
    try {
      created = await Prns.create({
        wasm: wasmModule(),
        identityStore: new BrowserLocalStorageIdentityStore(),
      });
    } catch (error: unknown) {
      return hostOperationFailed("Create runtime", error);
    }
    if (created.tag !== "Ready") {
      return created;
    }
    const registered = created.data.registerSingleDestination(
      BROWSER_PLAYGROUND_LXMF_DELIVERY.data.registration,
    );
    if (registered.tag !== "Registered") {
      return registered;
    }
    const pageRegistered = created.data.registerNodePage(
      new TextEncoder().encode(NODE_PAGE_DISPLAY_NAME),
    );
    if (pageRegistered.tag !== "Registered") {
      return pageRegistered;
    }
    const playground = new BrowserPlayground(
      view,
      created.data,
      registered.data,
      pageRegistered.data,
    );
    playground.#run();
    return Tag("Running", playground);
  }

  async close(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    if (this.#pollTimer !== undefined) {
      globalThis.clearInterval(this.#pollTimer);
      this.#pollTimer = undefined;
    }
    const usb = usbSession(this.#usb);
    const autoWifi =
      this.#autoWifi.tag === "Running"
        ? this.#autoWifi.data.controller
        : undefined;
    this.#usb = Tag("Closed");
    this.#autoWifi = Tag("Closed");
    await Promise.allSettled([usb?.close(), autoWifi?.close()]);
  }

  #run(): void {
    this.#autoWifi = Tag("Ready");
    this.#usb = webUsbAvailable()
      ? Tag("Ready")
      : Tag("Unavailable", { api: "WebUSB" });
    this.#view.renderRuntimeReady(this.#destination);
    this.#view.renderAutoWifi(this.#autoWifi);
    this.#view.renderUsb(this.#usb);
    this.#view.record(
      "Runtime",
      "Browser node runtime ready",
      `${LXMF_DELIVERY_DISPLAY_NAME} · lxmf.delivery ${hex(this.#destination)}`,
    );
    this.#view.record(
      "Node page",
      "Serving /page/index.mu over Reticulum",
      `${NODE_PAGE_DISPLAY_NAME} · nomadnetwork.node ${hex(this.#pageDestination)}`,
    );
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
    void this.#consumeEvents();
    void this.#consumeDiagnostics();
    this.#pollTimer = globalThis.setInterval(() => {
      this.#poll();
    }, POLL_INTERVAL_MS);
    this.#poll();
  }

  #startAutoWifi(): void {
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
    this.#view.record(
      "Auto Wi-Fi",
      "Discovery started",
      "Probing localhost, prns.local, and their local gateway catalogs.",
    );
    this.#syncControls();
  }

  async #closeAutoWifi(): Promise<void> {
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
          this.#view.record(
            "Failure",
            "Auto Wi-Fi close was rejected",
            describeRuntimeRejected(outcome),
          );
          break;
        default:
          this.#view.record(
            "Failure",
            "Auto Wi-Fi returned an unknown close outcome",
            describeUnknownOutcome("Auto Wi-Fi close", outcome),
          );
      }
    } catch (error: unknown) {
      const outcome = hostOperationFailed("Close Auto Wi-Fi", error);
      this.#view.record(
        "Failure",
        "Auto Wi-Fi close failed",
        describeHostOperationFailure(outcome),
      );
    }
    this.#pollAutoWifi();
    this.#view.renderAutoWifi(this.#autoWifi);
    this.#syncControls();
  }

  async #connectUsb(): Promise<void> {
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
    this.#view.record(
      "USB Auto",
      "Device selection opened",
      "Choose a Prns USB Auto device in the browser prompt.",
    );
    let outcome: UsbAutoConnectOutcome;
    try {
      outcome = await this.#prns.interfaces.usbAuto.connect();
    } catch (error: unknown) {
      const failure = hostOperationFailed("Connect USB Auto", error);
      this.#usb = Tag("ConnectFailed", failure);
      this.#view.renderUsb(this.#usb);
      this.#view.record(
        "Failure",
        "USB Auto did not connect",
        describeHostOperationFailure(failure),
      );
      this.#syncControls();
      return;
    }
    switch (outcome.tag) {
      case "Connected":
        this.#usb = Tag("Connected", outcome.data);
        this.#view.record(
          "USB Auto",
          "Session opened",
          `Interface ${hex(outcome.data.interfaceId)}`,
        );
        break;
      case "HostApiUnavailable":
      case "PermissionDenied":
      case "Cancelled":
      case "AlreadyActive":
      case "UnsupportedDevice":
      case "ConnectionFailed":
      case "RuntimeRejected":
        this.#usb = Tag("ConnectFailed", outcome);
        this.#view.record(
          "Failure",
          "USB Auto did not connect",
          describeUsbConnectFailure(outcome),
        );
        break;
      default:
        this.#view.record(
          "Failure",
          "USB Auto returned an unknown connection outcome",
          describeUnknownOutcome("USB Auto connection", outcome),
        );
    }
    this.#view.renderUsb(this.#usb);
    this.#syncControls();
  }

  async #closeUsb(): Promise<void> {
    const session = usbClosableSession(this.#usb);
    if (!session) {
      return;
    }
    this.#usb = Tag("Closing", session);
    this.#view.renderUsb(this.#usb);
    this.#syncControls();
    let outcome: InterfaceCloseOutcome;
    try {
      outcome = await session.close();
    } catch (error: unknown) {
      const failure = hostOperationFailed("Close USB Auto", error);
      this.#usb = Tag("CloseFailed", { session, failure });
      this.#view.renderUsb(this.#usb);
      this.#view.record(
        "Failure",
        "USB Auto close failed",
        describeHostOperationFailure(failure),
      );
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
        this.#view.record(
          "Failure",
          "USB Auto close failed",
          describeUsbCloseFailure(outcome),
        );
        break;
      default:
        this.#view.record(
          "Failure",
          "USB Auto returned an unknown close outcome",
          describeUnknownOutcome("USB Auto close", outcome),
        );
    }
    this.#view.renderUsb(this.#usb);
    this.#syncControls();
  }

  #announce(): void {
    if ((this.#snapshot?.interfaces.length ?? 0) === 0) {
      return;
    }
    void this.#announceDestination("LXMF delivery", this.#destination);
    void this.#announceDestination("Node page", this.#pageDestination);
  }

  async #announceDestination(
    label: string,
    destination: DestinationHash,
  ): Promise<void> {
    const outcome = await this.#prns.announce(destination);
    match(outcome, {
      Announced: () => {
        this.#view.record(
          "Announce",
          `${label} announce settled`,
          null,
        );
      },
      Busy: () => {
        this.#view.record(
          "Failure",
          `${label} announce was not accepted`,
          "The pending command limit is full",
        );
      },
      NodeStopped: () => {
        this.#view.record(
          "Failure",
          `${label} announce was not accepted`,
          "The node is no longer running",
        );
      },
      CommandFailed: ({ detail }) => {
        this.#view.record(
          "Failure",
          `${label} announce failed`,
          detail,
        );
      },
      HostApiUnavailable: ({ api }) => {
        this.#view.record(
          "Failure",
          `${label} announce was not accepted`,
          `${api} is unavailable in this browser`,
        );
      },
      EntropySourceFailed: ({ detail }) => {
        this.#view.record(
          "Failure",
          `${label} announce was not accepted`,
          detail,
        );
      },
      InsufficientEntropy: ({ actual, minimum }) => {
        this.#view.record(
          "Failure",
          `${label} announce was not accepted`,
          `${actual} bytes received; ${minimum} required`,
        );
      },
      RuntimeRejected: ({ operation, detail }) => {
        this.#view.record(
          "Failure",
          `${label} announce was rejected`,
          `${operation}: ${detail}`,
        );
      },
    });
  }

  #poll(): void {
    if (this.#closed) {
      return;
    }
    this.#pollAutoWifi();
    this.#pollUsb();
    this.#pollRuntime();
    this.#syncControls();
  }

  #pollAutoWifi(): void {
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
    } else {
      this.#autoWifi = Tag("Running", {
        controller: current.controller,
        status,
      });
    }
    this.#view.renderAutoWifi(this.#autoWifi);
    this.#recordAutoWifiStatus(status);
  }

  #recordAutoWifiStatus(status: AutoWifiControllerStatus): void {
    switch (status.tag) {
      case "Starting":
        this.#view.record("Auto Wi-Fi", "Transport starting", null);
        return;
      case "Discovering":
        this.#view.record(
          "Auto Wi-Fi",
          `Discovery attempt ${status.data.attempt}`,
          null,
        );
        return;
      case "Active":
        this.#view.record(
          "Auto Wi-Fi",
          `${status.data.gateways.length} gateway${status.data.gateways.length === 1 ? "" : "s"} active`,
          status.data.gateways.map((gateway) => gateway.url).join(" · "),
        );
        return;
      case "Unavailable":
        this.#view.record(
          "Failure",
          "Auto Wi-Fi is unavailable",
          describeAutoWifiFailure(status.data),
        );
        return;
      case "Closed":
        this.#view.record("Auto Wi-Fi", "Transport closed", null);
        return;
      default:
        this.#view.record(
          "Failure",
          "Auto Wi-Fi returned an unknown status",
          describeUnknownOutcome("Auto Wi-Fi status", status),
        );
    }
  }

  #pollUsb(): void {
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

  #pollRuntime(): void {
    const captured = this.#prns.snapshot();
    switch (captured.tag) {
      case "Captured":
        this.#snapshot = captured.data;
        this.#view.renderSnapshot(captured.data);
        this.#lastRuntimeFailure = "";
        return;
      case "RuntimeRejected":
        this.#recordRuntimeFailure(
          "Runtime snapshot was rejected",
          describeRuntimeRejected(captured),
        );
        return;
      default:
        this.#recordRuntimeFailure(
          "Runtime returned an unknown snapshot outcome",
          describeUnknownOutcome("snapshot", captured),
        );
    }
  }

  async #consumeEvents(): Promise<void> {
    try {
      for await (const event of this.#prns.events()) {
        if (this.#closed) {
          return;
        }
        this.#recordEvent(event);
      }
    } catch (error: unknown) {
      this.#recordRuntimeFailure(
        "Runtime application event stream failed",
        describeHostError(error),
      );
    }
  }

  async #consumeDiagnostics(): Promise<void> {
    try {
      for await (const event of this.#prns.diagnostics()) {
        if (this.#closed) {
          return;
        }
        this.#recordEvent(event);
      }
    } catch (error: unknown) {
      this.#recordRuntimeFailure(
        "Runtime diagnostic stream failed",
        describeHostError(error),
      );
    }
  }

  #recordEvent(event: PrnsEvent): void {
    match(event, {
      AnnounceHeard: ({ destination, hops, sourceInterface }) => {
        this.#view.record(
          "Network",
          "Announce received",
          `${hex(destination)} · ${hops} hop${hops === 1 ? "" : "s"} · interface ${hex(sourceInterface)}`,
        );
      },
      SingleDelivery: ({ destination, plaintext, sourceInterface }) => {
        const metadata = `destination ${hex(destination)} · interface ${hex(sourceInterface)}`;
        match(presentPacketContent(plaintext), {
          Empty: () => {
            this.#view.record(
              "Network",
              "Single packet received",
              `${metadata}\n(empty payload)`,
            );
          },
          Text: ({ value }) => {
            this.#view.record(
              "Network",
              "Single packet received",
              `${metadata}\n${value}`,
            );
          },
          Binary: ({ byteLength, hexadecimal }) => {
            this.#view.record(
              "Network",
              "Binary single packet received",
              `${metadata}\n${byteLength} bytes · ${hexadecimal}`,
            );
          },
        });
      },
      DiagnosticsDropped: ({ count }) => {
        this.#view.record(
          "Runtime",
          `${count.toString()} diagnostic event${count === 1n ? "" : "s"} dropped`,
          null,
        );
      },
      RouteExpired: ({ destination }) => {
        this.#view.record("Route", "Route expired", hex(destination));
      },
      RouteEvicted: ({ destination }) => {
        this.#view.record("Route", "Route evicted", hex(destination));
      },
      RouteInterfaceGone: ({ destination }) => {
        this.#view.record(
          "Route",
          "Route interface disappeared",
          hex(destination),
        );
      },
      RouteDropped: ({ destination }) => {
        this.#view.record("Route", "Route dropped", hex(destination));
      },
    });
  }

  #recordRuntimeFailure(summary: string, detail: string): void {
    const key = `${summary}:${detail}`;
    if (key === this.#lastRuntimeFailure) {
      return;
    }
    this.#lastRuntimeFailure = key;
    this.#view.record("Failure", summary, detail);
  }

  #syncControls(): void {
    this.#view.setControls(
      controlAvailability(this.#autoWifi, this.#usb, this.#snapshot),
    );
  }
}

async function boot(document: Document): Promise<void> {
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
  view.record(
    "Failure",
    "Browser node could not start",
    describeStartupFailure(startup),
  );
}

function usbSession(state: UsbState): UsbAutoSession | undefined {
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

function usbClosableSession(state: UsbState): UsbAutoSession | undefined {
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

function wasmModule(): PrnsWasmModule {
  // wasm-bindgen exposes byte newtypes as Uint8Array; this is the one boundary
  // where the SDK's branded views are attached to those generated bindings.
  return {
    PrnsRuntime: wasm.PrnsRuntime,
    UsbAutoDecoder: wasm.UsbAutoDecoder,
    BluetoothReassembler: wasm.BluetoothReassembler,
    hostContractAbi: wasm.hostContractAbi,
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
  } as unknown as PrnsWasmModule;
}

function webUsbAvailable(): boolean {
  return "usb" in navigator;
}

function unknownState(_state: never): undefined {
  return undefined;
}

void boot(document);

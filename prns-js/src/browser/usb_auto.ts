import { Tag } from "../casework.js";
import { connectFailure } from "./host_errors.js";
import { hostGlobal } from "./host_apis.js";
import type {
  BrowserUsb,
  BrowserUsbDevice,
  HostApiUnavailable,
} from "./host_apis.js";
import type {
  AlreadyActive,
  Cancelled,
  ConnectionFailed,
  InterfaceConnectStage,
  InterfaceId,
  InterfaceSession,
  PermissionDenied,
  RuntimeRejected,
  UnsupportedDevice,
} from "./index.js";
import type { UsbAutoRuntimeHost } from "./usb_auto/runtime.js";
import { BrowserUsbAutoSession } from "./usb_auto/session.js";
import {
  WebUsbAutoTransport,
  usbStage,
} from "./usb_auto/transport.js";
import { channelTag } from "./values.js";
import type { ChannelTag } from "./values.js";

export type UsbAutoSession = InterfaceSession & {
  readonly name: "usb-auto";
};

export type UsbAutoConnectOutcome =
  | Tag<"Connected", UsbAutoSession>
  | HostApiUnavailable<"WebUSB">
  | PermissionDenied<"usb-auto">
  | Cancelled<"usb-auto">
  | AlreadyActive<"usb-auto">
  | UnsupportedDevice<"usb-auto">
  | ConnectionFailed<"usb-auto">
  | RuntimeRejected;

export type UsbAutoDeviceFilter = {
  readonly vendorId?: number;
  readonly productId?: number;
  readonly serialNumber?: string;
};

export type UsbAutoConnectOptions = {
  readonly filters?: readonly UsbAutoDeviceFilter[];
};

let nextChannelTag = 0;

export class UsbAutoInterface {
  readonly name = "usb-auto" as const;
  readonly #host: UsbAutoRuntimeHost;

  constructor(host: UsbAutoRuntimeHost) {
    this.#host = host;
  }

  async connect(
    options: UsbAutoConnectOptions = {},
  ): Promise<UsbAutoConnectOutcome> {
    const ready = this.#host.runtimeReadiness();
    if (ready.tag !== "Ready") {
      return ready;
    }
    const available = requireWebUsb();
    if (available.tag !== "Available") {
      return available;
    }
    let transport: WebUsbAutoTransport | undefined;
    let interfaceId: InterfaceId | undefined;
    let stage: InterfaceConnectStage = "DeviceSelection";
    try {
      const requested = await usbStage("DeviceSelection", "request device", () =>
        available.data.requestDevice({
          filters: options.filters ?? this.#host.defaultUsbAutoFilters(),
        }),
      );
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
    } catch (error) {
      if (interfaceId) {
        this.#host.deactivateInterface(interfaceId);
      }
      await transport?.close();
      return connectFailure("usb-auto", stage, error);
    }
  }
}

function requireWebUsb():
  | Tag<"Available", BrowserUsb>
  | HostApiUnavailable<"WebUSB"> {
  try {
    const usb = hostGlobal().navigator?.usb;
    return usb
      ? Tag("Available", usb)
      : Tag("HostApiUnavailable", { api: "WebUSB" });
  } catch {
    return Tag("HostApiUnavailable", { api: "WebUSB" });
  }
}

function browserUsbAutoChannelTag(device: BrowserUsbDevice): ChannelTag {
  const vendor = formatOptionalHex(device.vendorId);
  const product = formatOptionalHex(device.productId);
  const serial = device.serialNumber ?? "unknown";
  const nonce = nextChannelTag;
  nextChannelTag = (nextChannelTag + 1) >>> 0;
  return channelTag(
    new TextEncoder().encode(`webusb:auto-usb:${vendor}:${product}:${serial}:${nonce}`),
  );
}

function formatOptionalHex(value: number | undefined): string {
  return value === undefined ? "unknown" : value.toString(16).padStart(4, "0");
}

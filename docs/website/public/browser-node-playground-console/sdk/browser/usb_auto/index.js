import { Tag } from "../../casework.js";
import { connectFailure } from "../host_errors.js";
import { hostGlobal } from "../host_apis.js";
import { BrowserUsbAutoSession } from "./session.js";
import { WebUsbAutoTransport, usbStage, } from "./transport.js";
import { channelTag } from "../values.js";
let nextChannelTag = 0;
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
function browserUsbAutoChannelTag(device) {
    const vendor = formatOptionalHex(device.vendorId);
    const product = formatOptionalHex(device.productId);
    const serial = device.serialNumber ?? "unknown";
    const nonce = nextChannelTag;
    nextChannelTag = (nextChannelTag + 1) >>> 0;
    return channelTag(new TextEncoder().encode(`webusb:auto-usb:${vendor}:${product}:${serial}:${nonce}`));
}
function formatOptionalHex(value) {
    return value === undefined ? "unknown" : value.toString(16).padStart(4, "0");
}

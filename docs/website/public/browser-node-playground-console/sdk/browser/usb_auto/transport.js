import { Tag } from "../../casework.js";
import { describeHostError, domExceptionName } from "../host_errors.js";
const MIN_TRANSFER_BYTES = 512;
const LINUX_SETUP_HINT = "On Linux, run ./tools/prns device webusb install from the Prns repo root, " +
    "then unplug/replug the device and restart the browser. If this is Snap Chromium, " +
    "also run sudo snap connect chromium:raw-usb or use a non-Snap Chrome/Chromium build.";
export class WebUsbAutoTransport {
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
        const opened = await usbStage("TransportOpen", "open selected device", () => device.open());
        if (opened.tag !== "Completed") {
            return opened;
        }
        const configured = firstUsbConfiguration(device);
        if (configured.tag !== "Configured") {
            await closeUsbDevice(device);
            return configured;
        }
        const configuration = device.configuration ?? configured.data;
        if (!device.configuration) {
            const selected = await usbStage("TransportOpen", `select configuration ${configuration.configurationValue}`, () => device.selectConfiguration(configuration.configurationValue));
            if (selected.tag !== "Completed") {
                await closeUsbDevice(device);
                return selected;
            }
        }
        const selectedConfiguration = device.configuration ?? configured.data;
        const endpoints = findWebUsbEndpointPair(selectedConfiguration);
        if (!endpoints) {
            await closeUsbDevice(device);
            return Tag("UnsupportedDevice", {
                interface: "usb-auto",
                capability: "usable IN/OUT endpoint pair",
            });
        }
        const claimed = await usbStage("TransportOpen", `claim interface ${endpoints.interfaceNumber}`, () => device.claimInterface(endpoints.interfaceNumber));
        if (claimed.tag !== "Completed") {
            await closeUsbDevice(device);
            return claimed;
        }
        if (endpoints.alternate.alternateSetting !== 0 &&
            device.selectAlternateInterface) {
            const selected = await usbStage("TransportOpen", `select alternate ${endpoints.alternate.alternateSetting} ` +
                `on interface ${endpoints.interfaceNumber}`, () => device.selectAlternateInterface(endpoints.interfaceNumber, endpoints.alternate.alternateSetting));
            if (selected.tag !== "Completed") {
                await closeUsbDevice(device);
                return selected;
            }
        }
        return Tag("Opened", new WebUsbAutoTransport(device, endpoints.interfaceNumber, endpoints.inEndpoint, endpoints.outEndpoint));
    }
    async read() {
        if (this.#closed) {
            return Tag("Read", undefined);
        }
        try {
            const length = Math.max(this.#inEndpoint.packetSize, MIN_TRANSFER_BYTES);
            const result = await this.#device.transferIn(this.#inEndpoint.endpointNumber, length);
            if (result.status !== "ok") {
                return Tag("TransferFailed", {
                    direction: "Inbound",
                    detail: `USB transfer status ${result.status}`,
                });
            }
            const data = result.data;
            if (!data) {
                return Tag("Read", new Uint8Array());
            }
            return Tag("Read", new Uint8Array(data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength)));
        }
        catch (error) {
            return Tag("TransferFailed", {
                direction: "Inbound",
                detail: describeHostError(error),
            });
        }
    }
    async write(bytes) {
        if (this.#closed || bytes.length === 0) {
            return Tag("Written");
        }
        try {
            const result = await this.#device.transferOut(this.#outEndpoint.endpointNumber, arrayBuffer(bytes));
            if (result.status !== "ok" || result.bytesWritten !== bytes.length) {
                return Tag("TransferFailed", {
                    direction: "Outbound",
                    detail: `wrote ${result.bytesWritten}/${bytes.length} bytes with status ${result.status}`,
                });
            }
            return Tag("Written");
        }
        catch (error) {
            return Tag("TransferFailed", {
                direction: "Outbound",
                detail: describeHostError(error),
            });
        }
    }
    async close() {
        if (this.#closed) {
            return [];
        }
        this.#closed = true;
        const failures = [];
        try {
            await this.#device.releaseInterface(this.#interfaceNumber);
        }
        catch (error) {
            failures.push(Tag("TransportCloseFailed", {
                detail: `release USB interface: ${describeHostError(error)}`,
            }));
        }
        try {
            await this.#device.close();
        }
        catch (error) {
            failures.push(Tag("TransportCloseFailed", {
                detail: `close USB device: ${describeHostError(error)}`,
            }));
        }
        return failures;
    }
}
export async function usbStage(stage, actionName, action) {
    try {
        return Tag("Completed", await action());
    }
    catch (error) {
        const name = domExceptionName(error);
        if (name === "SecurityError" || name === "NotAllowedError") {
            return Tag("PermissionDenied", {
                interface: "usb-auto",
                stage,
                detail: describeUsbError(error, actionName),
            });
        }
        if (name === "NotFoundError" && stage === "DeviceSelection") {
            return Tag("Cancelled", { interface: "usb-auto", stage });
        }
        return Tag("ConnectionFailed", {
            interface: "usb-auto",
            stage,
            detail: `USB ${actionName} failed: ${describeUsbError(error, actionName)}`,
        });
    }
}
function firstUsbConfiguration(device) {
    const configuration = device.configurations[0];
    if (!configuration) {
        return Tag("UnsupportedDevice", {
            interface: "usb-auto",
            capability: "USB configuration",
        });
    }
    return Tag("Configured", configuration);
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
function describeUsbError(error, stage) {
    const base = describeHostError(error);
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
        return `${base}. ${LINUX_SETUP_HINT}`;
    }
    if (name === "NotFoundError" && stage.includes("request")) {
        return `${base}. No USB device was selected.`;
    }
    return base;
}
async function closeUsbDevice(device) {
    try {
        await device.close();
        return undefined;
    }
    catch (error) {
        return Tag("TransportCloseFailed", {
            detail: `close USB device: ${describeHostError(error)}`,
        });
    }
}
function arrayBuffer(bytes) {
    const out = new ArrayBuffer(bytes.length);
    new Uint8Array(out).set(bytes);
    return out;
}

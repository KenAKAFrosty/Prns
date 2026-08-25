import { Tag } from "../../casework.js";
import { connectFailure } from "../host_errors.js";
import { hostGlobal } from "../host_apis.js";
import { bluetoothStage, disconnectBluetoothServer, } from "./gatt.js";
import { BrowserBluetoothSession } from "./session.js";
export class BluetoothInterface {
    name = "bluetooth";
    #host;
    constructor(host) {
        this.#host = host;
    }
    async connect() {
        const identity = this.#host.bluetoothIdentityReadiness();
        if (identity.tag !== "Ready") {
            return identity;
        }
        const ready = this.#host.runtimeReadiness();
        if (ready.tag !== "Ready") {
            return ready;
        }
        const available = requireWebBluetooth();
        if (available.tag !== "Available") {
            return available;
        }
        let server;
        let session;
        let stage = "DeviceSelection";
        try {
            const serviceUuid = this.#host.bluetoothServiceUuid();
            const requested = await bluetoothStage("DeviceSelection", () => available.data.requestDevice({
                filters: [{ services: [serviceUuid] }],
                optionalServices: [serviceUuid],
            }));
            if (requested.tag !== "Completed") {
                return requested;
            }
            const gatt = requested.data.gatt;
            if (!gatt) {
                return Tag("UnsupportedDevice", {
                    interface: "bluetooth",
                    capability: "GATT server",
                });
            }
            stage = "TransportOpen";
            const connected = await bluetoothStage("TransportOpen", () => gatt.connect());
            if (connected.tag !== "Completed") {
                return connected;
            }
            const connectedServer = connected.data;
            server = connectedServer;
            stage = "ServiceDiscovery";
            const discovered = await bluetoothStage("ServiceDiscovery", () => connectedServer.getPrimaryService(serviceUuid));
            if (discovered.tag !== "Completed") {
                disconnectBluetoothServer(connectedServer);
                return discovered;
            }
            const control = await bluetoothStage("ServiceDiscovery", () => discovered.data.getCharacteristic(this.#host.bluetoothControlUuid()));
            if (control.tag !== "Completed") {
                disconnectBluetoothServer(connectedServer);
                return control;
            }
            const data = await optionalBluetoothCharacteristic(discovered.data, this.#host.bluetoothDataUuid());
            stage = "Handshake";
            session = new BrowserBluetoothSession(this.#host, connectedServer, control.data, data ?? control.data);
            const started = await session.start();
            if (started.tag !== "Started") {
                await session.close();
                return started;
            }
            return Tag("Connected", session);
        }
        catch (error) {
            if (session) {
                await session.close();
            }
            else if (server) {
                disconnectBluetoothServer(server);
            }
            return connectFailure("bluetooth", stage, error);
        }
    }
}
function requireWebBluetooth() {
    try {
        const bluetooth = hostGlobal().navigator?.bluetooth;
        return bluetooth
            ? Tag("Available", bluetooth)
            : Tag("HostApiUnavailable", { api: "WebBluetooth" });
    }
    catch {
        return Tag("HostApiUnavailable", { api: "WebBluetooth" });
    }
}
async function optionalBluetoothCharacteristic(service, uuid) {
    try {
        return await service.getCharacteristic(uuid);
    }
    catch {
        return undefined;
    }
}

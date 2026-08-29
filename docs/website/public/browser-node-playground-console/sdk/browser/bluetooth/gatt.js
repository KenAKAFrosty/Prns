import { Tag } from "../../casework.js";
import { describeHostError, domExceptionName } from "../host_errors.js";
export async function bluetoothStage(stage, action) {
    try {
        return Tag("Completed", await action());
    }
    catch (error) {
        return bluetoothStageFailure(stage, error);
    }
}
export async function optionalBluetoothCharacteristic(service, uuid) {
    try {
        return Tag("Completed", await service.getCharacteristic(uuid));
    }
    catch (error) {
        return domExceptionName(error) === "NotFoundError"
            ? Tag("Completed", undefined)
            : bluetoothStageFailure("ServiceDiscovery", error);
    }
}
export function disconnectBluetoothServer(server) {
    try {
        server.disconnect();
        return undefined;
    }
    catch (error) {
        return Tag("TransportCloseFailed", {
            detail: `disconnect Bluetooth server: ${describeHostError(error)}`,
        });
    }
}
export function characteristicBytes(event) {
    const value = event.target?.value;
    if (!value) {
        return Tag("ProtocolViolation", {
            protocol: "Bluetooth",
            detail: "Bluetooth characteristic event did not include a value",
        });
    }
    return Tag("Decoded", new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
}
export async function writeBluetoothValue(characteristic, bytes) {
    const value = arrayBuffer(bytes);
    try {
        if (characteristic.properties.write &&
            characteristic.writeValueWithResponse) {
            await characteristic.writeValueWithResponse(value);
        }
        else if (characteristic.properties.writeWithoutResponse &&
            characteristic.writeValueWithoutResponse) {
            await characteristic.writeValueWithoutResponse(value);
        }
        else if ((characteristic.properties.write ||
            characteristic.properties.writeWithoutResponse) &&
            characteristic.writeValue) {
            await characteristic.writeValue(value);
        }
        else {
            return Tag("TransferFailed", {
                direction: "Outbound",
                detail: "Bluetooth characteristic does not support writes",
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
function arrayBuffer(bytes) {
    const out = new ArrayBuffer(bytes.length);
    new Uint8Array(out).set(bytes);
    return out;
}
function bluetoothStageFailure(stage, error) {
    const name = domExceptionName(error);
    if (name === "SecurityError" || name === "NotAllowedError") {
        return Tag("PermissionDenied", {
            interface: "bluetooth",
            stage,
            detail: describeHostError(error),
        });
    }
    if (name === "NotFoundError" && stage === "DeviceSelection") {
        return Tag("Cancelled", { interface: "bluetooth", stage });
    }
    return Tag("ConnectionFailed", {
        interface: "bluetooth",
        stage,
        detail: describeHostError(error),
    });
}

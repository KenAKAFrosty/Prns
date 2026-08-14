import { Tag } from "../../casework.js";
import { describeHostError, domExceptionName } from "../host_errors.js";
import type {
  BrowserBluetoothCharacteristicEvent,
  BrowserBluetoothRemoteGattCharacteristic,
  BrowserBluetoothRemoteGattServer,
} from "../host_apis.js";
import type {
  Cancelled,
  ConnectionFailed,
  InterfaceCleanupFailure,
  InterfaceConnectStage,
  InterfaceSessionFailure,
  PermissionDenied,
} from "../interface_contract.js";

export type BluetoothStageOutcome<Value> =
  | Tag<"Completed", Value>
  | PermissionDenied<"bluetooth">
  | Cancelled<"bluetooth">
  | ConnectionFailed<"bluetooth">;

export type BluetoothWriteOutcome =
  | Tag<"Written">
  | InterfaceSessionFailure;

export type CharacteristicBytesOutcome =
  | Tag<"Decoded", Uint8Array>
  | Extract<InterfaceSessionFailure, Tag<"ProtocolViolation", unknown>>;

export async function bluetoothStage<T>(
  stage: InterfaceConnectStage,
  action: () => Promise<T>,
): Promise<BluetoothStageOutcome<T>> {
  try {
    return Tag("Completed", await action());
  } catch (error) {
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
}

export function disconnectBluetoothServer(
  server: BrowserBluetoothRemoteGattServer,
): InterfaceCleanupFailure | undefined {
  try {
    server.disconnect();
    return undefined;
  } catch (error) {
    return Tag("TransportCloseFailed", {
      detail: `disconnect Bluetooth server: ${describeHostError(error)}`,
    });
  }
}

export function characteristicBytes(
  event: BrowserBluetoothCharacteristicEvent,
): CharacteristicBytesOutcome {
  const value = event.target?.value;
  if (!value) {
    return Tag("ProtocolViolation", {
      protocol: "Bluetooth",
      detail: "Bluetooth characteristic event did not include a value",
    });
  }
  return Tag(
    "Decoded",
    new Uint8Array(value.buffer, value.byteOffset, value.byteLength),
  );
}

export async function writeBluetoothValue(
  characteristic: BrowserBluetoothRemoteGattCharacteristic,
  bytes: Uint8Array,
): Promise<BluetoothWriteOutcome> {
  const value = arrayBuffer(bytes);
  try {
    if (characteristic.writeValueWithoutResponse) {
      await characteristic.writeValueWithoutResponse(value);
    } else if (characteristic.writeValueWithResponse) {
      await characteristic.writeValueWithResponse(value);
    } else if (characteristic.writeValue) {
      await characteristic.writeValue(value);
    } else {
      return Tag("TransferFailed", {
        direction: "Outbound",
        detail: "Bluetooth characteristic does not support writes",
      });
    }
    return Tag("Written");
  } catch (error) {
    return Tag("TransferFailed", {
      direction: "Outbound",
      detail: describeHostError(error),
    });
  }
}

function arrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

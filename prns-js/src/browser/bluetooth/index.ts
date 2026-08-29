import { Tag } from "../../casework.js";
import { connectFailure } from "../host_errors.js";
import { hostGlobal } from "../host_apis.js";
import type {
  BrowserBluetooth,
  BrowserBluetoothRemoteGattServer,
  HostApiUnavailable,
} from "../host_apis.js";
import {
  bluetoothStage,
  disconnectBluetoothServer,
  optionalBluetoothCharacteristic,
} from "./gatt.js";
import type { BluetoothRuntimeHost } from "./runtime.js";
import { BrowserBluetoothSession } from "./session.js";
import type {
  AlreadyActive,
  Cancelled,
  ConnectionFailed,
  ConnectTimedOut,
  InterfaceConnectStage,
  InterfaceSession,
  PermissionDenied,
  UnsupportedDevice,
} from "../interface_contract.js";
import type {
  RuntimeRejected,
  StableIdentityUnavailable,
} from "../runtime_contract.js";

export type BluetoothSession = InterfaceSession & {
  readonly name: "bluetooth";
};

export type BluetoothConnectOutcome =
  | Tag<"Connected", BluetoothSession>
  | HostApiUnavailable<"WebBluetooth">
  | PermissionDenied<"bluetooth">
  | Cancelled<"bluetooth">
  | UnsupportedDevice<"bluetooth">
  | ConnectTimedOut<"bluetooth">
  | ConnectionFailed<"bluetooth">
  | AlreadyActive<"bluetooth">
  | StableIdentityUnavailable<"bluetooth">
  | RuntimeRejected;

export type BluetoothConnectFailure = Exclude<
  BluetoothConnectOutcome,
  Tag<"Connected", unknown>
>;

export class BluetoothInterface {
  readonly name = "bluetooth" as const;
  readonly #host: BluetoothRuntimeHost;

  constructor(host: BluetoothRuntimeHost) {
    this.#host = host;
  }

  async connect(): Promise<BluetoothConnectOutcome> {
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
    let server: BrowserBluetoothRemoteGattServer | undefined;
    let session: BrowserBluetoothSession | undefined;
    let stage: InterfaceConnectStage = "DeviceSelection";
    try {
      const serviceUuid = this.#host.bluetoothServiceUuid();
      const requested = await bluetoothStage(
        "DeviceSelection",
        () =>
          available.data.requestDevice({
            filters: [{ services: [serviceUuid] }],
            optionalServices: [serviceUuid],
          }),
      );
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
      const connected = await bluetoothStage(
        "TransportOpen",
        () => gatt.connect(),
      );
      if (connected.tag !== "Completed") {
        return connected;
      }
      const connectedServer = connected.data;
      server = connectedServer;
      stage = "ServiceDiscovery";
      const discovered = await bluetoothStage(
        "ServiceDiscovery",
        () => connectedServer.getPrimaryService(serviceUuid),
      );
      if (discovered.tag !== "Completed") {
        disconnectBluetoothServer(connectedServer);
        return discovered;
      }
      const control = await bluetoothStage(
        "ServiceDiscovery",
        () =>
          discovered.data.getCharacteristic(this.#host.bluetoothControlUuid()),
      );
      if (control.tag !== "Completed") {
        disconnectBluetoothServer(connectedServer);
        return control;
      }
      const data = await optionalBluetoothCharacteristic(
        discovered.data,
        this.#host.bluetoothDataUuid(),
      );
      if (data.tag !== "Completed") {
        disconnectBluetoothServer(connectedServer);
        return data;
      }
      stage = "Handshake";
      session = new BrowserBluetoothSession(
        this.#host,
        requested.data,
        connectedServer,
        control.data,
        data.data ?? control.data,
      );
      const started = await session.start();
      if (started.tag !== "Started") {
        await session.close();
        return started;
      }
      return Tag("Connected", session);
    } catch (error) {
      if (session) {
        await session.close();
      } else if (server) {
        disconnectBluetoothServer(server);
      }
      return connectFailure("bluetooth", stage, error);
    }
  }
}

function requireWebBluetooth():
  | Tag<"Available", BrowserBluetooth>
  | HostApiUnavailable<"WebBluetooth"> {
  try {
    const bluetooth = hostGlobal().navigator?.bluetooth;
    return bluetooth && typeof bluetooth.requestDevice === "function"
      ? Tag("Available", bluetooth)
      : Tag("HostApiUnavailable", { api: "WebBluetooth" });
  } catch {
    return Tag("HostApiUnavailable", { api: "WebBluetooth" });
  }
}

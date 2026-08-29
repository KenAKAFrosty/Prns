import { Tag, match_into } from "./sdk/index.js";
import type {
  AutoWifiController,
  AutoWifiControllerStatus,
  AutoWifiFailure,
  AutoWifiGatewayStatus,
  BluetoothSession,
  InterfaceSessionFailure,
  PrnsSnapshot,
  Tag as Tagged,
  UsbAutoSession,
  WebSocketSession,
} from "./sdk/index.js";
import type {
  BluetoothConnectFailure,
  HostOperationFailed,
  InterfaceCloseFailure,
  UsbConnectFailure,
  WebSocketConnectFailure,
} from "./outcomes.js";

export type BluetoothState =
  | Tagged<"Waiting">
  | Tagged<"Ready">
  | Tagged<"Unavailable", { readonly api: "WebBluetooth" }>
  | Tagged<"Connecting">
  | Tagged<"Session", BluetoothSession>
  | Tagged<"SessionFailed", InterfaceSessionFailure>
  | Tagged<"Closing", BluetoothSession>
  | Tagged<"ConnectFailed", BluetoothConnectFailure | HostOperationFailed>
  | Tagged<"Closed">
  | Tagged<
      "CloseFailed",
      {
        readonly session: BluetoothSession;
        readonly failure: InterfaceCloseFailure | HostOperationFailed;
      }
    >;

export type AutoWifiState =
  | Tagged<"Waiting">
  | Tagged<"Ready">
  | Tagged<
      "Running",
      {
        readonly controller: AutoWifiController;
        readonly status: AutoWifiControllerStatus;
      }
    >
  | Tagged<"Closed">;

export type UsbState =
  | Tagged<"Waiting">
  | Tagged<"Ready">
  | Tagged<"Unavailable", { readonly api: "WebUSB" }>
  | Tagged<"Connecting">
  | Tagged<"Connected", UsbAutoSession>
  | Tagged<"Closing", UsbAutoSession>
  | Tagged<"ConnectFailed", UsbConnectFailure | HostOperationFailed>
  | Tagged<"Closed">
  | Tagged<
      "CloseFailed",
      {
        readonly session: UsbAutoSession;
        readonly failure: InterfaceCloseFailure | HostOperationFailed;
      }
    >;

export type WebSocketState =
  | Tagged<"Waiting">
  | Tagged<"Ready">
  | Tagged<"Unavailable", { readonly api: "WebSocket" }>
  | Tagged<"Connecting", { readonly url: string }>
  | Tagged<"Connected", WebSocketSession>
  | Tagged<"Closing", WebSocketSession>
  | Tagged<
      "ConnectFailed",
      WebSocketConnectFailure | HostOperationFailed
    >
  | Tagged<"Closed">
  | Tagged<
      "CloseFailed",
      {
        readonly session: WebSocketSession;
        readonly failure: InterfaceCloseFailure | HostOperationFailed;
      }
    >;

export type ControlAvailability = {
  readonly autoWifiStart: boolean;
  readonly autoWifiClose: boolean;
  readonly webSocketConnect: boolean;
  readonly webSocketClose: boolean;
  readonly usbConnect: boolean;
  readonly usbClose: boolean;
  readonly bluetoothConnect: boolean;
  readonly bluetoothClose: boolean;
  readonly announce: boolean;
};

export function controlAvailability(
  autoWifi: AutoWifiState,
  webSocket: WebSocketState,
  usb: UsbState,
  bluetooth: BluetoothState,
  snapshot: PrnsSnapshot | undefined,
): ControlAvailability {
  return {
    autoWifiStart: autoWifiStartAvailable(autoWifi),
    autoWifiClose: autoWifiCloseAvailable(autoWifi),
    webSocketConnect: webSocketConnectAvailable(webSocket),
    webSocketClose: webSocketCloseAvailable(webSocket),
    usbConnect: usbConnectAvailable(usb),
    usbClose: usbCloseAvailable(usb),
    bluetoothConnect: bluetoothConnectAvailable(bluetooth),
    bluetoothClose: bluetoothCloseAvailable(bluetooth),
    announce: (snapshot?.interfaces.length ?? 0) > 0,
  };
}

export function bluetoothConnectAvailable(state: BluetoothState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => true,
    Unavailable: () => false,
    Connecting: () => false,
    Session: (session) =>
      session.status.tag === "Closed" || session.status.tag === "Failed",
    SessionFailed: () => true,
    Closing: () => false,
    ConnectFailed: () => true,
    Closed: () => true,
    CloseFailed: () => false,
  });
}

function bluetoothCloseAvailable(state: BluetoothState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => false,
    Unavailable: () => false,
    Connecting: () => false,
    Session: (session) =>
      session.status.tag === "Negotiating" || session.status.tag === "Active",
    SessionFailed: () => false,
    Closing: () => false,
    ConnectFailed: () => false,
    Closed: () => false,
    CloseFailed: () => true,
  });
}

export function bluetoothSession(
  state: BluetoothState,
): BluetoothSession | undefined {
  return match_into<BluetoothSession | undefined>().from(state, {
    Waiting: () => undefined,
    Ready: () => undefined,
    Unavailable: () => undefined,
    Connecting: () => undefined,
    Session: (session) => session,
    SessionFailed: () => undefined,
    Closing: (session) => session,
    ConnectFailed: () => undefined,
    Closed: () => undefined,
    CloseFailed: ({ session }) => session,
  });
}

export function bluetoothClosableSession(
  state: BluetoothState,
): BluetoothSession | undefined {
  return match_into<BluetoothSession | undefined>().from(state, {
    Waiting: () => undefined,
    Ready: () => undefined,
    Unavailable: () => undefined,
    Connecting: () => undefined,
    Session: (session) =>
      session.status.tag === "Negotiating" || session.status.tag === "Active"
        ? session
        : undefined,
    SessionFailed: () => undefined,
    Closing: () => undefined,
    ConnectFailed: () => undefined,
    Closed: () => undefined,
    CloseFailed: ({ session }) => session,
  });
}

export function observeBluetoothSession(
  session: BluetoothSession,
):
  | Tagged<"Live">
  | Tagged<"Closed">
  | Tagged<"Failed", InterfaceSessionFailure> {
  return match_into<
    | Tagged<"Live">
    | Tagged<"Closed">
    | Tagged<"Failed", InterfaceSessionFailure>
  >().from(session.status, {
    Negotiating: () => Tag("Live"),
    Active: () => Tag("Live"),
    Closed: () => Tag("Closed"),
    Failed: (failure) => Tag("Failed", failure),
  });
}

export function webSocketConnectAvailable(state: WebSocketState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => true,
    Unavailable: () => false,
    Connecting: () => false,
    Connected: () => false,
    Closing: () => false,
    ConnectFailed: () => true,
    Closed: () => true,
    CloseFailed: () => false,
  });
}

function webSocketCloseAvailable(state: WebSocketState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => false,
    Unavailable: () => false,
    Connecting: () => false,
    Connected: () => true,
    Closing: () => false,
    ConnectFailed: () => false,
    Closed: () => false,
    CloseFailed: () => true,
  });
}

export function sameAutoWifiStatus(
  left: AutoWifiControllerStatus,
  right: AutoWifiControllerStatus,
): boolean {
  return match_into<boolean>().from(left, {
    Starting: () => right.tag === "Starting",
    Discovering: ({ attempt }) =>
      right.tag === "Discovering" && attempt === right.data.attempt,
    Active: ({ gateways }) =>
      right.tag === "Active" && sameGateways(gateways, right.data.gateways),
    Unavailable: (failure) =>
      right.tag === "Unavailable" &&
      sameAutoWifiFailure(failure, right.data),
    Closed: () => right.tag === "Closed",
  });
}

function autoWifiStartAvailable(state: AutoWifiState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => true,
    Running: () => false,
    Closed: () => true,
  });
}

function autoWifiCloseAvailable(state: AutoWifiState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => false,
    Running: () => true,
    Closed: () => false,
  });
}

function usbConnectAvailable(state: UsbState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => true,
    Unavailable: () => false,
    Connecting: () => false,
    Connected: () => false,
    Closing: () => false,
    ConnectFailed: () => true,
    Closed: () => true,
    CloseFailed: () => false,
  });
}

function usbCloseAvailable(state: UsbState): boolean {
  return match_into<boolean>().from(state, {
    Waiting: () => false,
    Ready: () => false,
    Unavailable: () => false,
    Connecting: () => false,
    Connected: () => true,
    Closing: () => false,
    ConnectFailed: () => false,
    Closed: () => false,
    CloseFailed: () => true,
  });
}

function sameGateways(
  left: readonly AutoWifiGatewayStatus[],
  right: readonly AutoWifiGatewayStatus[],
): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((gateway, index) => {
    const candidate = right[index];
    return (
      candidate !== undefined &&
      gateway.id === candidate.id &&
      gateway.url === candidate.url &&
      gateway.localhost === candidate.localhost &&
      sameBytes(gateway.interfaceId, candidate.interfaceId)
    );
  });
}

function sameAutoWifiFailure(
  left: AutoWifiFailure,
  right: AutoWifiFailure,
): boolean {
  return match_into<boolean>().from(left, {
    HostApiUnavailable: ({ api }) =>
      right.tag === "HostApiUnavailable" && api === right.data.api,
    PermissionDenied: ({ interface: interfaceName, stage, detail }) =>
      right.tag === "PermissionDenied" &&
      interfaceName === right.data.interface &&
      stage === right.data.stage &&
      detail === right.data.detail,
    AlreadyActive: ({ interface: interfaceName, target }) =>
      right.tag === "AlreadyActive" &&
      interfaceName === right.data.interface &&
      target === right.data.target,
    SelectionIdentityUnavailable: ({ detail }) =>
      right.tag === "SelectionIdentityUnavailable" &&
      detail === right.data.detail,
    DiscoveryFailed: ({ detail }) =>
      right.tag === "DiscoveryFailed" && detail === right.data.detail,
    RuntimeRejected: ({ operation, detail }) =>
      right.tag === "RuntimeRejected" &&
      operation === right.data.operation &&
      detail === right.data.detail,
  });
}

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((byte, index) => byte === right[index]);
}

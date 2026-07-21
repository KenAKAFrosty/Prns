import type {
  AutoWifiController,
  AutoWifiControllerStatus,
  AutoWifiFailure,
  AutoWifiGatewayStatus,
  PrnsSnapshot,
  Tag as Tagged,
  UsbAutoSession,
} from "./sdk/index.js";
import type {
  HostOperationFailed,
  UsbCloseFailure,
  UsbConnectFailure,
} from "./outcomes.js";

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
        readonly failure: UsbCloseFailure | HostOperationFailed;
      }
    >;

export type ControlAvailability = {
  readonly autoWifiStart: boolean;
  readonly autoWifiClose: boolean;
  readonly usbConnect: boolean;
  readonly usbClose: boolean;
  readonly announce: boolean;
};

export function controlAvailability(
  autoWifi: AutoWifiState,
  usb: UsbState,
  snapshot: PrnsSnapshot | undefined,
): ControlAvailability {
  return {
    autoWifiStart: autoWifiStartAvailable(autoWifi),
    autoWifiClose: autoWifiCloseAvailable(autoWifi),
    usbConnect: usbConnectAvailable(usb),
    usbClose: usbCloseAvailable(usb),
    announce: (snapshot?.interfaces.length ?? 0) > 0,
  };
}

export function sameAutoWifiStatus(
  left: AutoWifiControllerStatus,
  right: AutoWifiControllerStatus,
): boolean {
  switch (left.tag) {
    case "Starting":
      return right.tag === "Starting";
    case "Discovering":
      return (
        right.tag === "Discovering" &&
        left.data.attempt === right.data.attempt
      );
    case "Active":
      return (
        right.tag === "Active" &&
        sameGateways(left.data.gateways, right.data.gateways)
      );
    case "Unavailable":
      return (
        right.tag === "Unavailable" &&
        sameAutoWifiFailure(left.data, right.data)
      );
    case "Closed":
      return right.tag === "Closed";
    default:
      return sameUnknownAutoWifiStatus(left);
  }
}

function autoWifiStartAvailable(state: AutoWifiState): boolean {
  switch (state.tag) {
    case "Waiting":
    case "Running":
      return false;
    case "Ready":
    case "Closed":
      return true;
    default:
      return unavailableForUnknownState(state);
  }
}

function autoWifiCloseAvailable(state: AutoWifiState): boolean {
  switch (state.tag) {
    case "Waiting":
    case "Ready":
    case "Closed":
      return false;
    case "Running":
      return true;
    default:
      return unavailableForUnknownState(state);
  }
}

function usbConnectAvailable(state: UsbState): boolean {
  switch (state.tag) {
    case "Ready":
    case "ConnectFailed":
    case "Closed":
      return true;
    case "Waiting":
    case "Unavailable":
    case "Connecting":
    case "Connected":
    case "Closing":
    case "CloseFailed":
      return false;
    default:
      return unavailableForUnknownState(state);
  }
}

function usbCloseAvailable(state: UsbState): boolean {
  switch (state.tag) {
    case "Connected":
    case "CloseFailed":
      return true;
    case "Waiting":
    case "Ready":
    case "Unavailable":
    case "Connecting":
    case "Closing":
    case "ConnectFailed":
    case "Closed":
      return false;
    default:
      return unavailableForUnknownState(state);
  }
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
  if (left.tag !== right.tag) {
    return false;
  }
  switch (left.tag) {
    case "HostApiUnavailable":
      return right.tag === left.tag && left.data.api === right.data.api;
    case "PermissionDenied":
      return (
        right.tag === left.tag &&
        left.data.interface === right.data.interface &&
        left.data.stage === right.data.stage &&
        left.data.detail === right.data.detail
      );
    case "AlreadyActive":
      return (
        right.tag === left.tag &&
        left.data.interface === right.data.interface &&
        left.data.target === right.data.target
      );
    case "SelectionIdentityUnavailable":
    case "DiscoveryFailed":
      return right.tag === left.tag && left.data.detail === right.data.detail;
    case "RuntimeRejected":
      return (
        right.tag === left.tag &&
        left.data.operation === right.data.operation &&
        left.data.detail === right.data.detail
      );
    default:
      return sameUnknownAutoWifiFailure(left);
  }
}

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((byte, index) => byte === right[index]);
}

function unavailableForUnknownState(_state: never): false {
  return false;
}

function sameUnknownAutoWifiStatus(_status: never): false {
  return false;
}

function sameUnknownAutoWifiFailure(_failure: never): false {
  return false;
}

import { DevelopmentNodeRuntime } from "@prns-internal/expo";
import type { DevelopmentRuntimeView } from "@/native/development-runtime-context";

type StatusInput = Pick<
  DevelopmentRuntimeView,
  | "availability"
  | "snapshot"
  | "phase"
  | "androidRuntime"
  | "bluetoothAuthorization"
  | "bluetoothAuthorizationFailure"
>;

export type BluetoothPresentation = {
  readonly label: string;
  readonly description: string;
  readonly warning?: boolean;
  readonly showConnectedPeers?: boolean;
};

/** Device access and actual physical connections are independent of node health. */
export function bluetoothStatus(runtime: StatusInput): BluetoothPresentation {
  if (runtime.availability.type === "unavailable") {
    return { label: "Unavailable", description: "Nearby connections need the iOS or Android app." };
  }
  const bluetooth = runtime.snapshot?.bluetooth;
  if (bluetooth?.desiredEnabled === false) {
    return bluetooth.state.tag === "Disabling"
      ? { label: "Turning off", description: "Disconnecting nearby devices…" }
      : { label: "Off", description: "Bluetooth connections are turned off for this app." };
  }
  if (runtime.snapshot?.runtime === DevelopmentNodeRuntime.Stopped) {
    return {
      label: "Node stopped",
      description: "Start this phone's node to connect to nearby devices.",
    };
  }
  const android = runtime.androidRuntime;
  const authorization = runtime.bluetoothAuthorization?.authorization;
  if (android?.failure || runtime.bluetoothAuthorizationFailure) {
    return {
      label: "Status unavailable",
      description: "Bluetooth access could not be checked. Refresh to try again.",
      warning: true,
    };
  }
  if (
    (runtime.availability.platform === "android" && !android?.status) ||
    (runtime.availability.platform === "ios" && authorization === undefined)
  ) {
    return { label: "Checking access", description: "Checking Bluetooth availability…" };
  }
  if (android?.status?.bluetoothRadio === "unsupported" || authorization === "restricted") {
    return {
      label: "Unavailable",
      description:
        authorization === "restricted"
          ? "Bluetooth access is restricted on this phone."
          : "Bluetooth is not supported on this phone.",
      warning: true,
    };
  }
  if (
    (android?.status !== null &&
      android?.status !== undefined &&
      android.status.bluetoothPermission !== "granted") ||
    authorization === "denied" ||
    authorization === "notDetermined"
  ) {
    return {
      label: "Permission needed",
      description:
        "Allow Bluetooth access to connect to nearby devices. Your saved data is available.",
      warning: true,
    };
  }
  if (android?.status?.bluetoothRadio === "off") {
    return {
      label: "Bluetooth is off",
      description: "Turn on Bluetooth in your phone's settings.",
      warning: true,
    };
  }
  if (android?.status?.locationServices === "off") {
    return {
      label: "Location needed",
      description:
        "Turn on Location in Android Settings so this version of Android can discover nearby devices.",
      warning: true,
    };
  }
  if (android?.status?.bluetoothRadio === "unknown") {
    return { label: "Checking access", description: "Checking Bluetooth availability…" };
  }
  if (bluetooth === undefined || bluetooth.desiredEnabled === undefined) {
    return {
      label: runtime.phase === "failed" ? "Unavailable" : "Checking",
      description:
        runtime.phase === "failed"
          ? "This phone's connection settings could not be loaded."
          : "Checking Bluetooth connections…",
      warning: runtime.phase === "failed",
    };
  }
  switch (bluetooth.state.tag) {
    case "Stopped":
      return {
        label: "Node stopped",
        description: "Start this phone's node to connect to nearby devices.",
      };
    case "Starting":
      return { label: "Starting", description: "Preparing Bluetooth connections…" };
    case "Connecting":
      return { label: "Connecting", description: "Establishing a nearby Bluetooth connection…" };
    case "RadioOff":
      return {
        label: "Bluetooth is off",
        description: "Turn on Bluetooth in your phone's settings, then return to prns.",
        warning: true,
      };
    case "Disabled":
    case "Disabling":
      return { label: "Updating", description: "Applying the Bluetooth setting…" };
    case "Connected": {
      const count = bluetooth.peers.filter((peer) => peer.connected).length;
      if (count > 0)
        return {
          label: `Connected to ${count} ${count === 1 ? "device" : "devices"}`,
          description:
            "These are nearby Bluetooth connections. Messages can also reach destinations through them.",
          showConnectedPeers: true,
        };
      return { label: "Updating", description: "Checking the current Bluetooth connections…" };
    }
    case "WaitingForPeers":
      return {
        label: "Ready",
        description:
          "No devices connected. Connections form automatically when compatible devices are nearby.",
      };
    case "Unavailable":
      return {
        label: "Unavailable",
        description:
          "Bluetooth connections are unavailable. Check access below and refresh; diagnostics have more detail.",
        warning: true,
      };
    default:
      return { label: "Checking", description: "Checking Bluetooth connections…" };
  }
}

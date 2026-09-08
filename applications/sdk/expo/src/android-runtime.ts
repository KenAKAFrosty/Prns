import { NativePayloadError } from "./hydrate";
import type { AndroidRuntimeNativeModule } from "./native";

export type AndroidRuntimeStatus = {
  readonly revision: number;
  readonly bluetoothPermission: "notRequested" | "granted" | "denied" | "blocked";
  readonly backgroundDiscovery: "notRequired" | "granted" | "notGranted";
  readonly bluetoothRadio: "unknown" | "unsupported" | "off" | "on";
  readonly locationServices: "notRequired" | "on" | "off";
  readonly service: "stopped" | "starting" | "running" | "stopping" | "failed";
  readonly lastError: string | null;
};

export type AndroidRuntime = {
  readonly readStatus: () => Promise<AndroidRuntimeStatus>;
  readonly requestBluetoothPermissions: () => Promise<AndroidRuntimeStatus>;
  readonly requestBackgroundBluetoothPermission: () => Promise<AndroidRuntimeStatus>;
  readonly addStatusListener: (listener: (status: AndroidRuntimeStatus) => void) => {
    readonly remove: () => void;
  };
};

export function createAndroidRuntime(nativeModule: AndroidRuntimeNativeModule): AndroidRuntime {
  return {
    readStatus: async () => parseAndroidRuntimeStatus(await nativeModule.androidRuntimeStatus()),
    requestBluetoothPermissions: async () =>
      parseAndroidRuntimeStatus(await nativeModule.requestBluetoothPermissions()),
    requestBackgroundBluetoothPermission: async () =>
      parseAndroidRuntimeStatus(await nativeModule.requestBackgroundBluetoothPermission()),
    addStatusListener: (listener) =>
      nativeModule.addListener("onAndroidRuntimeStatus", (event) => {
        try {
          listener(parseAndroidRuntimeStatus(event.status));
        } catch {
          // A malformed event cannot grant access. The explicit read reports errors.
        }
      }),
  };
}

export function parseAndroidRuntimeStatus(json: string): AndroidRuntimeStatus {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    throw new NativePayloadError("$", "Android runtime status is not valid JSON");
  }
  if (
    value === null ||
    typeof value !== "object" ||
    !("revision" in value) ||
    !Number.isSafeInteger(value.revision) ||
    typeof value.revision !== "number" ||
    value.revision < 0 ||
    !("bluetoothPermission" in value) ||
    !oneOf(value.bluetoothPermission, ["notRequested", "granted", "denied", "blocked"]) ||
    !("backgroundDiscovery" in value) ||
    !oneOf(value.backgroundDiscovery, ["notRequired", "granted", "notGranted"]) ||
    !("bluetoothRadio" in value) ||
    !oneOf(value.bluetoothRadio, ["unknown", "unsupported", "off", "on"]) ||
    !("locationServices" in value) ||
    !oneOf(value.locationServices, ["notRequired", "on", "off"]) ||
    !("service" in value) ||
    !oneOf(value.service, ["stopped", "starting", "running", "stopping", "failed"]) ||
    !("lastError" in value) ||
    !(value.lastError === null || typeof value.lastError === "string")
  ) {
    throw new NativePayloadError("$", "Android runtime status has an invalid shape");
  }
  return {
    revision: value.revision,
    bluetoothPermission: value.bluetoothPermission,
    backgroundDiscovery: value.backgroundDiscovery,
    bluetoothRadio: value.bluetoothRadio,
    locationServices: value.locationServices,
    service: value.service,
    lastError: value.lastError,
  };
}

function oneOf<const Value extends string>(
  value: unknown,
  options: readonly Value[],
): value is Value {
  return typeof value === "string" && options.some((option) => option === value);
}

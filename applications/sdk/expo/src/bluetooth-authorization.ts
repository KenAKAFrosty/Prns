import { NativePayloadError } from "./native-payload";
import type { AppleBluetoothNativeModule } from "./native";

export type BluetoothAuthorizationStatus = {
  readonly authorization: "notDetermined" | "allowedAlways" | "denied" | "restricted";
  readonly nativeStart: "failed" | "notRequested" | "running" | "starting" | "stopping";
  readonly restorationAttemptRequested: boolean;
  readonly revision: number;
};

export type BluetoothAuthorizationRuntime = {
  readonly readStatus: () => Promise<BluetoothAuthorizationStatus>;
  readonly addStatusListener: (listener: (status: BluetoothAuthorizationStatus) => void) => {
    readonly remove: () => void;
  };
};

const authorizations = new Set<BluetoothAuthorizationStatus["authorization"]>([
  "notDetermined",
  "allowedAlways",
  "denied",
  "restricted",
]);
const nativeStartPhases = new Set<BluetoothAuthorizationStatus["nativeStart"]>([
  "failed",
  "notRequested",
  "running",
  "starting",
  "stopping",
]);

export function createBluetoothAuthorizationRuntime(
  nativeModule: AppleBluetoothNativeModule,
): BluetoothAuthorizationRuntime {
  return {
    readStatus: async () =>
      parseBluetoothAuthorizationStatus(await nativeModule.bluetoothAuthorizationStatus()),
    addStatusListener: (listener) =>
      nativeModule.addListener("onBluetoothAuthorizationStatus", (event) => {
        try {
          listener(parseBluetoothAuthorizationStatus(event.status));
        } catch {
          // Explicit reads report malformed state; malformed events must not
          // crash the React tree.
        }
      }),
  };
}

export function parseBluetoothAuthorizationStatus(json: string): BluetoothAuthorizationStatus {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch (cause) {
    throw new NativePayloadError("$", "Bluetooth authorization status is not valid JSON", {
      cause,
    });
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new NativePayloadError("$", "Bluetooth authorization status must be an object");
  }
  const { authorization, nativeStart, restorationAttemptRequested, revision } = parsed as Record<
    string,
    unknown
  >;
  if (
    typeof authorization !== "string" ||
    !authorizations.has(authorization as BluetoothAuthorizationStatus["authorization"])
  ) {
    throw new NativePayloadError("$.authorization", "Bluetooth authorization is invalid");
  }
  if (
    typeof nativeStart !== "string" ||
    !nativeStartPhases.has(nativeStart as BluetoothAuthorizationStatus["nativeStart"])
  ) {
    throw new NativePayloadError("$.nativeStart", "native start phase is invalid");
  }
  if (typeof restorationAttemptRequested !== "boolean") {
    throw new NativePayloadError(
      "$.restorationAttemptRequested",
      "restoration attempt marker is invalid",
    );
  }
  if (!Number.isSafeInteger(revision) || (revision as number) < 0) {
    throw new NativePayloadError("$.revision", "Bluetooth authorization revision is invalid");
  }
  return {
    authorization: authorization as BluetoothAuthorizationStatus["authorization"],
    nativeStart: nativeStart as BluetoothAuthorizationStatus["nativeStart"],
    restorationAttemptRequested,
    revision: revision as number,
  };
}

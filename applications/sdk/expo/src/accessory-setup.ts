import { NativePayloadError } from "./native-payload";
import type { AppleAccessorySetupNativeModule } from "./native";

export type AccessorySetupStatus = {
  readonly phase: "activating" | "failed" | "ready" | "setupRequired";
  readonly picker: "idle" | "presented" | "presenting";
  readonly authorizedAccessoryCount: number;
  readonly nativeStart: "failed" | "notRequested" | "running" | "starting" | "stopping";
  readonly restorationLaunchRequested: boolean;
  readonly revision: number;
  readonly lastError: {
    readonly code: string;
    readonly detail: string;
  } | null;
};

export type AccessorySetupPickerOutcome = {
  readonly type:
    | "alreadyActive"
    | "cancelled"
    | "completed"
    | "failed"
    | "notReady"
    | "restricted"
    | "timedOut";
};

export type AccessorySetupSubscription = { readonly remove: () => void };

export type AccessorySetupRuntime = {
  readonly readStatus: () => Promise<AccessorySetupStatus>;
  readonly showPicker: () => Promise<AccessorySetupPickerOutcome>;
  readonly addStatusListener: (
    listener: (status: AccessorySetupStatus) => void,
  ) => AccessorySetupSubscription;
};

const phases = new Set<AccessorySetupStatus["phase"]>([
  "activating",
  "failed",
  "ready",
  "setupRequired",
]);
const pickerPhases = new Set<AccessorySetupStatus["picker"]>(["idle", "presented", "presenting"]);
const nativeStartPhases = new Set<AccessorySetupStatus["nativeStart"]>([
  "failed",
  "notRequested",
  "running",
  "starting",
  "stopping",
]);
const pickerOutcomes = new Set<AccessorySetupPickerOutcome["type"]>([
  "alreadyActive",
  "cancelled",
  "completed",
  "failed",
  "notReady",
  "restricted",
  "timedOut",
]);

export function createAccessorySetupRuntime(
  nativeModule: AppleAccessorySetupNativeModule,
): AccessorySetupRuntime {
  return {
    readStatus: async () => parseStatus(await nativeModule.accessorySetupStatus()),
    showPicker: async () => parsePickerOutcome(await nativeModule.showAccessorySetupPicker()),
    addStatusListener: (listener) =>
      nativeModule.addListener("onAccessorySetupStatus", (event) => {
        try {
          listener(parseStatus(event.status));
        } catch {
          // The explicit read path reports malformed native state. Ignore a bad
          // event rather than letting an event callback crash the React tree.
        }
      }),
  };
}

export function parseAccessorySetupStatus(json: string): AccessorySetupStatus {
  return parseStatus(json);
}

function parseStatus(json: string): AccessorySetupStatus {
  const value = parseObject(json, "accessory setup status");
  const {
    phase,
    picker,
    authorizedAccessoryCount,
    nativeStart,
    restorationLaunchRequested,
    revision,
  } = value;
  if (typeof phase !== "string" || !phases.has(phase as AccessorySetupStatus["phase"])) {
    throw new NativePayloadError("$.phase", "accessory setup phase is invalid");
  }
  if (typeof picker !== "string" || !pickerPhases.has(picker as AccessorySetupStatus["picker"])) {
    throw new NativePayloadError("$.picker", "accessory setup picker phase is invalid");
  }
  if (!Number.isSafeInteger(authorizedAccessoryCount) || (authorizedAccessoryCount as number) < 0) {
    throw new NativePayloadError(
      "$.authorizedAccessoryCount",
      "authorized accessory count is invalid",
    );
  }
  if (
    typeof nativeStart !== "string" ||
    !nativeStartPhases.has(nativeStart as AccessorySetupStatus["nativeStart"])
  ) {
    throw new NativePayloadError("$.nativeStart", "native start phase is invalid");
  }
  if (typeof restorationLaunchRequested !== "boolean") {
    throw new NativePayloadError(
      "$.restorationLaunchRequested",
      "restoration launch marker is invalid",
    );
  }
  if (!Number.isSafeInteger(revision) || (revision as number) < 0) {
    throw new NativePayloadError("$.revision", "accessory setup revision is invalid");
  }
  const lastError = parseLastError(value.lastError);
  return {
    phase: phase as AccessorySetupStatus["phase"],
    picker: picker as AccessorySetupStatus["picker"],
    authorizedAccessoryCount: authorizedAccessoryCount as number,
    nativeStart: nativeStart as AccessorySetupStatus["nativeStart"],
    restorationLaunchRequested,
    revision: revision as number,
    lastError,
  };
}

function parsePickerOutcome(json: string): AccessorySetupPickerOutcome {
  const value = parseObject(json, "accessory setup picker outcome");
  if (
    typeof value.type !== "string" ||
    !pickerOutcomes.has(value.type as AccessorySetupPickerOutcome["type"])
  ) {
    throw new NativePayloadError("$.type", "accessory setup picker outcome is invalid");
  }
  return { type: value.type as AccessorySetupPickerOutcome["type"] };
}

function parseLastError(value: unknown): AccessorySetupStatus["lastError"] {
  if (value === null) {
    return null;
  }
  if (typeof value !== "object" || Array.isArray(value)) {
    throw new NativePayloadError("$.lastError", "accessory setup error is invalid");
  }
  const error = value as Record<string, unknown>;
  if (typeof error.code !== "string" || typeof error.detail !== "string") {
    throw new NativePayloadError("$.lastError", "accessory setup error fields are invalid");
  }
  return { code: error.code, detail: error.detail };
}

function parseObject(json: string, label: string): Record<string, unknown> {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch (cause) {
    throw new NativePayloadError("$", `${label} is not valid JSON`, { cause });
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new NativePayloadError("$", `${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

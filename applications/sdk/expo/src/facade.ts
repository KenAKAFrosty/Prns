import { NATIVE_CONTRACT_FINGERPRINT } from "./contract.generated";
import type {
  DescribeRemoteControlTargetInput as WireDescribeRemoteControlTargetInput,
  DevelopmentNodeSnapshot as WireDevelopmentNodeSnapshot,
  DevelopmentNodeStartOutcome as WireDevelopmentNodeStartOutcome,
  DevelopmentNodeStopOutcome as WireDevelopmentNodeStopOutcome,
  InitiateRemoteControlPairingInput as WireInitiateRemoteControlPairingInput,
  RemoteControlDescribeOutcome as WireRemoteControlDescribeOutcome,
  RemoteControlPairingCommandOutcome as WireRemoteControlPairingCommandOutcome,
  RemoteControlPairingDecisionInput as WireRemoteControlPairingDecisionInput,
} from "./contract.generated";
import { hydrateGenerated, NativePayloadError, type Hydrated } from "./hydrate";
import type { PrnsAppNativeModule } from "./native";

type BridgeFailure = {
  readonly type: "bridgeFailure";
  readonly kind: "invalidInput" | "panic";
  readonly detail: string;
};

export type DevelopmentNodeSnapshot = Hydrated<WireDevelopmentNodeSnapshot>;
export type DevelopmentNodeStartOutcome = Hydrated<WireDevelopmentNodeStartOutcome>;
export type DevelopmentNodeStopOutcome = Hydrated<WireDevelopmentNodeStopOutcome>;
export type RemoteControlPairingCommandOutcome = Hydrated<WireRemoteControlPairingCommandOutcome>;
export type RemoteControlDescribeOutcome = Hydrated<WireRemoteControlDescribeOutcome>;
export type InitiateRemoteControlPairingInput = Hydrated<WireInitiateRemoteControlPairingInput>;
export type RemoteControlPairingDecisionInput = Hydrated<WireRemoteControlPairingDecisionInput>;
export type DescribeRemoteControlTargetInput = Hydrated<WireDescribeRemoteControlTargetInput>;

export type DevelopmentRuntime = {
  readonly startDevelopmentNode: () => Promise<DevelopmentNodeStartOutcome>;
  readonly readDevelopmentNodeSnapshot: () => Promise<DevelopmentNodeSnapshot>;
  readonly initiateRemoteControlPairing: (
    input: InitiateRemoteControlPairingInput,
  ) => Promise<RemoteControlPairingCommandOutcome>;
  readonly approveRemoteControlPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Promise<RemoteControlPairingCommandOutcome>;
  readonly rejectRemoteControlPairing: (
    input: RemoteControlPairingDecisionInput,
  ) => Promise<RemoteControlPairingCommandOutcome>;
  readonly describeRemoteControlTarget: (
    input: DescribeRemoteControlTargetInput,
  ) => Promise<RemoteControlDescribeOutcome>;
  readonly stopDevelopmentNode: () => Promise<DevelopmentNodeStopOutcome>;
  readonly resetDevelopmentData: () => Promise<DevelopmentNodeStopOutcome>;
};

export class NativeContractMismatchError extends Error {
  readonly expected: string;
  readonly actual: string;

  constructor(expected: string, actual: string) {
    super(`Native contract mismatch: expected ${expected}, received ${actual}`);
    this.name = "NativeContractMismatchError";
    this.expected = expected;
    this.actual = actual;
  }
}

export class NativeBridgeError extends Error {
  readonly kind: BridgeFailure["kind"];

  constructor(failure: BridgeFailure) {
    super(`Native bridge ${failure.kind}: ${failure.detail}`);
    this.name = "NativeBridgeError";
    this.kind = failure.kind;
  }
}

export function createDevelopmentRuntime(nativeModule: PrnsAppNativeModule): DevelopmentRuntime {
  let handshake: Promise<void> | undefined;

  const verifyContract = (): Promise<void> => {
    if (handshake === undefined) {
      handshake = nativeModule
        .contractFingerprint()
        .then((actual) => {
          if (actual !== NATIVE_CONTRACT_FINGERPRINT) {
            throw new NativeContractMismatchError(NATIVE_CONTRACT_FINGERPRINT, actual);
          }
        })
        .catch((error: unknown) => {
          handshake = undefined;
          throw error;
        });
    }
    return handshake;
  };

  const read = async <Wire>(operation: () => Promise<string>): Promise<Hydrated<Wire>> => {
    await verifyContract();
    const result = parseBridgeJson<Wire>(await operation());
    verifyEmbeddedContractFingerprints(result);
    return hydrateGenerated(result);
  };

  return {
    startDevelopmentNode: () => read(() => nativeModule.start()),
    readDevelopmentNodeSnapshot: () => read(() => nativeModule.snapshot()),
    initiateRemoteControlPairing: (input) =>
      read(() => nativeModule.initiatePairing(JSON.stringify(input))),
    approveRemoteControlPairing: (input) =>
      read(() => nativeModule.approvePairing(JSON.stringify(input))),
    rejectRemoteControlPairing: (input) =>
      read(() => nativeModule.rejectPairing(JSON.stringify(input))),
    describeRemoteControlTarget: (input) =>
      read(() =>
        nativeModule.describeTarget(
          JSON.stringify({
            targetIdentityFingerprint: Array.from(input.targetIdentityFingerprint),
          } satisfies WireDescribeRemoteControlTargetInput),
        ),
      ),
    stopDevelopmentNode: () => read(() => nativeModule.stop()),
    resetDevelopmentData: () => read(() => nativeModule.reset()),
  };
}

function parseBridgeJson<Wire>(json: string): Wire {
  let parsed: Wire | BridgeFailure;
  try {
    parsed = JSON.parse(json) as Wire | BridgeFailure;
  } catch (cause) {
    throw new NativePayloadError("$", "native result is not valid JSON", { cause });
  }
  if (isBridgeFailure(parsed)) {
    throw new NativeBridgeError(parsed);
  }
  return parsed;
}

function isBridgeFailure(value: unknown): value is BridgeFailure {
  return (
    value !== null &&
    typeof value === "object" &&
    "type" in value &&
    value.type === "bridgeFailure" &&
    "kind" in value &&
    (value.kind === "invalidInput" || value.kind === "panic") &&
    "detail" in value &&
    typeof value.detail === "string"
  );
}

function verifyEmbeddedContractFingerprints(value: unknown): void {
  if (Array.isArray(value)) {
    for (const item of value) {
      verifyEmbeddedContractFingerprints(item);
    }
    return;
  }
  if (value === null || typeof value !== "object") {
    return;
  }
  for (const [key, nested] of Object.entries(value)) {
    if (key === "contractFingerprint") {
      if (nested !== NATIVE_CONTRACT_FINGERPRINT) {
        throw new NativeContractMismatchError(
          NATIVE_CONTRACT_FINGERPRINT,
          typeof nested === "string" ? nested : String(nested),
        );
      }
    } else {
      verifyEmbeddedContractFingerprints(nested);
    }
  }
}
